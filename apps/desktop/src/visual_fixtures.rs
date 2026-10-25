use super::*;
use wor_infrastructure::device::{NodeIdentity, NodeKind};

const SCENES: &[(&str, Page, bool, bool, bool)] = &[
    ("image", Page::Image, false, false, false),
    ("drive", Page::Drive, false, false, false),
    ("options", Page::Options, false, false, false),
    ("review", Page::Review, false, false, false),
    ("progress", Page::Progress, false, false, false),
    ("result", Page::Result, false, false, false),
    ("boot-editor", Page::Options, false, true, false),
    ("boot-advanced", Page::Options, false, true, true),
    ("iot-options", Page::Options, true, false, false),
    ("iot-result", Page::Result, true, false, false),
    ("error", Page::Review, false, false, false),
];

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) struct RenderHandles {
    pub document: Signal<Value>,
    pub appearance: Signal<Appearance>,
    pub scope: ScopeId,
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(super) struct RenderProbe(pub std::rc::Rc<std::cell::RefCell<Option<RenderHandles>>>);

#[cfg(test)]
#[derive(Clone, Copy)]
pub(super) struct BootHandles {
    pub context: Signal<Value>,
    pub advanced: Signal<bool>,
    pub computed: Memo<std::result::Result<Value, String>>,
    pub scope: ScopeId,
}

#[cfg(test)]
#[derive(Clone, Default)]
pub(super) struct BootProbe(pub std::rc::Rc<std::cell::RefCell<Option<BootHandles>>>);

fn service(root: &Path, scene: &str, appearance: Appearance) -> anyhow::Result<Arc<Services>> {
    let (_, page, iot, boot_editor, advanced_editor) = SCENES
        .iter()
        .find(|(name, ..)| *name == scene)
        .context("Unknown visual fixture")?;
    let mut document: Value =
        serde_json::from_slice(&std::fs::read(root.join("config-templates/config.json"))?)?;
    if *iot {
        switch_family(&mut document, true);
        set_value(&mut document, "target.board", Some(json!("pi3-b")));
        set_value(&mut document, "target.rpiModel", Some(json!(3)));
        set_value(
            &mut document,
            "userAccount.iotCore.enabled",
            Some(json!(true)),
        );
        set_value(
            &mut document,
            "userAccount.iotCore.currentUsername",
            Some(json!("Administrator")),
        );
        set_value(
            &mut document,
            "userAccount.iotCore.currentPassword",
            Some(json!("VisualFixture_Only42!")),
        );
        set_value(
            &mut document,
            "userAccount.iotCore.password",
            Some(json!("VisualFixture_Only42!")),
        );
    } else {
        set_value(&mut document, "userAccount.setupAccount", Some(json!(true)));
        set_value(
            &mut document,
            "userAccount.username",
            Some(json!("VisualUser")),
        );
        set_value(
            &mut document,
            "userAccount.password",
            Some(json!("VisualFixture_Only42!")),
        );
    }
    set_value(&mut document, "execution.dryRun", Some(json!(true)));
    set_value(
        &mut document,
        "execution.confirmIotErase",
        Some(json!(false)),
    );
    set_value(
        &mut document,
        "execution.confirmIotWipe",
        Some(json!(false)),
    );
    set_value(
        &mut document,
        "target.device",
        Some(json!("/dev/visual-fixture-a")),
    );
    set_value(
        &mut document,
        "media.sourceFile",
        Some(json!(if *iot {
            "/Visual fixtures/reviewed-example.ffu"
        } else {
            "/Visual fixtures/Windows ARM64 example with a deliberately long source path for responsive layout.iso"
        })),
    );
    let node = NodeIdentity {
        device: 0,
        inode: 0,
        rdev: 0,
        kind: NodeKind::File,
    };
    let devices = [("a", 64_u64), ("b", 128_u64)]
        .into_iter()
        .map(|(name, gib)| DeviceSnapshot {
            path: format!("/dev/visual-fixture-{name}").into(),
            raw_path: format!("/dev/visual-fixture-{name}").into(),
            bytes: gib * 1024 * 1024 * 1024,
            sector_size: 512,
            layout: "fixture-not-a-device".into(),
            model: format!("Visual fixture USB {gib} GiB - no physical device"),
            node: node.clone(),
            raw_node: node.clone(),
        })
        .collect();
    let selection = Selection {
        config: root.join("config-templates/config.json"),
        device: "/dev/visual-fixture-a".into(),
        dry_run: true,
    };
    let preview = Preview {
        selection, config_digest: "0".repeat(64), target_fingerprint: None,
        summary: "VISUAL FIXTURE ONLY - not write authorization\nImage: Windows ARM\nBoard: Raspberry Pi\nTarget: /dev/visual-fixture-a (not a device)\nMode: dry run\nImage verification: required\nHardware boot: Not Assessed\nCredentials: masked and private".into(),
    };
    let mut view = View {
        devices,
        busy: *page == Page::Progress,
        percent: if *page == Page::Progress { 42.0 } else { 100.0 },
        status: "Visual fixture - no imaging operation was executed.".into(),
        step: "Example verification status".into(),
        task: "Example frozen payload readback".into(),
        details: "Fixture diagnostics only.\nNo sudo, downloads, SSH, disks or native engine are available.\nHardware and account status: Not Assessed.".into(),
        ..View::default()
    };
    if scene == "error" {
        view.error = "Example authorization/validation failure: permission was not granted. No operation started and no drive was written.".into();
    }
    if *iot && *page == Page::Result {
        view.account_probe = Some(json!({
            "address":"192.0.2.42", "keyType":"ssh-ed25519", "key":"fixture-only",
            "fingerprint":"SHA256:VisualFixtureOnlyNotARealIdentity"
        }));
        view.account_result = Some(json!({
            "state":"pending-reboot", "username":"VisualUser", "address":"192.0.2.42",
            "accountState":"not-assessed", "languageState":"pending-reboot", "language":"en-US"
        }));
    }
    let (sender, receiver) = mpsc::unbounded_channel();
    Ok(Arc::new(Services {
        engine: None,
        root: root.to_owned(),
        metadata: serde_json::from_slice(&std::fs::read(root.join("src/config/metadata.json"))?)?,
        fields: prepare_fields(serde_json::from_slice(&std::fs::read(
            root.join("config-templates/config.schema.json"),
        )?)?),
        host_locale: "en_US.UTF-8".into(),
        initial: document,
        presentation: Presentation {
            page: *page,
            view,
            preview: Some(preview),
            boot_editor: *boot_editor,
            advanced_editor: *advanced_editor,
            appearance,
        },
        appearance_path: None,
        visual_fixture: true,
        draft: tempfile::tempdir()?,
        sender,
        receiver: Mutex::new(Some(receiver)),
        cancellation: Mutex::new(None),
        busy: AtomicBool::new(false),
        closing: AtomicBool::new(false),
        smoke: false,
        alert_auto: AtomicBool::new(false),
        alert_monitor: Mutex::new(None),
        alert_warning: Arc::new(Mutex::new(None)),
        #[cfg(test)]
        ui_resolutions: std::sync::atomic::AtomicUsize::new(0),
    }))
}

pub(super) fn export(root: &Path, output: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(output)?;
    if std::fs::symlink_metadata(output)?.file_type().is_symlink() {
        anyhow::bail!("Visual fixture output must not be a symlink.");
    }
    let assets = output.join("assets");
    std::fs::create_dir_all(&assets)?;
    if std::fs::symlink_metadata(&assets)?.file_type().is_symlink() {
        anyhow::bail!("Visual fixture assets must not be a linked directory.");
    }
    let mut artwork = tempfile::NamedTempFile::new_in(&assets)?;
    std::io::Write::write_all(
        artwork.as_file_mut(),
        &std::fs::read(root.join("assets/partnership.png"))?,
    )?;
    artwork.persist(assets.join("partnership.png"))?;
    let manifest = json!({
        "schemaVersion":1,
        "source":"actual native Dioxus App and shared stylesheet",
        "nativeOperationsAvailable":false,
        "scenes":SCENES.iter().map(|(name,..)| *name).collect::<Vec<_>>(),
        "themes":["system","light","dark"]
    });
    std::fs::write(
        output.join("fixtures.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    std::fs::write(
        output.join("fixture-driver.js"),
        include_str!("fixture-driver.js"),
    )?;
    for (scene, ..) in SCENES {
        for appearance in [Appearance::System, Appearance::Light, Appearance::Dark] {
            let services = service(root, scene, appearance)?;
            let mut dom = VirtualDom::new(App);
            dom.provide_root_context(services);
            dom.rebuild_in_place();
            let rendered = dioxus_ssr::render(&dom);
            let mut navigation = String::new();
            for (name, ..) in SCENES {
                navigation.push_str(&format!(
                    "<option value=\"{name}\"{}>{name}</option>",
                    if name == scene { " selected" } else { "" }
                ));
            }
            let html = format!(
                "<!doctype html><html lang=\"en\" data-fixture-scene=\"{scene}\"><head>\
                 <meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
                 <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'; script-src 'self'; img-src 'self' data:; connect-src 'none'; base-uri 'none'; form-action 'none'\">\
                 <title>WoR-Flasher visual fixture: {scene} / {appearance}</title>\
                 <style>.fixture-banner{{font:14px system-ui;padding:12px;background:Canvas;color:CanvasText;border-bottom:1px solid GrayText;display:flex;gap:12px;align-items:center;flex-wrap:wrap}}.fixture-banner select{{min-height:44px;max-width:100%}}</style>\
                 </head><body><aside class=\"fixture-banner\" aria-label=\"Visual fixture controls\">\
                 <strong>Production UI rendering; fixture data only. No native operations.</strong>\
                 <label for=\"fixture-scene\">Fixture screen</label><select id=\"fixture-scene\">{navigation}</select>\
                 </aside>{rendered}<script src=\"fixture-driver.js\" defer></script></body></html>"
            );
            std::fs::write(output.join(format!("{scene}-{appearance}.html")), html)?;
        }
    }
    println!(
        "Exported {} inert production-UI fixtures to {}",
        SCENES.len() * 3,
        output.display()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn partnership_fixture_artwork_is_exact_local_data_and_refuses_linked_directories() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let output = tempfile::tempdir().expect("fixture output");
        export(&root, output.path()).expect("export");
        assert_eq!(
            std::fs::read(output.path().join("assets/partnership.png")).expect("exported artwork"),
            std::fs::read(root.join("assets/partnership.png")).expect("canonical artwork")
        );
        let html = std::fs::read_to_string(output.path().join("image-light.html")).expect("markup");
        assert!(html.contains("src=\"assets/partnership.png\""));
        assert!(html.contains("Original creator"));
        assert!(html.contains("Community maintainer"));

        let external = tempfile::tempdir().expect("unrelated directory");
        let marker = external.path().join("partnership.png");
        std::fs::write(&marker, b"unrelated data").expect("unrelated fixture");
        let linked = tempfile::tempdir().expect("linked output");
        std::os::unix::fs::symlink(external.path(), linked.path().join("assets")).expect("link");
        assert!(export(&root, linked.path()).is_err());
        assert_eq!(
            std::fs::read(marker).expect("unrelated data remains"),
            b"unrelated data"
        );
    }

    #[test]
    fn visual_services_have_no_engine_or_native_operation_channel() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let services = service(&root, "image", Appearance::System).unwrap();
        assert!(services.engine.is_none());
        assert!(services.visual_fixture);
        assert!(services.appearance_path.is_none());
        services.send(Action::Inventory);
        services.send(Action::Close);
        assert!(
            services
                .receiver
                .lock()
                .unwrap()
                .as_mut()
                .unwrap()
                .try_recv()
                .is_err()
        );
        assert!(!services.alert_auto.load(Ordering::Acquire));
    }

    #[test]
    fn rendered_app_reuses_config_and_schema_until_the_document_really_changes() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        UI_SCHEMA_PARSES.with(|count| count.set(0));
        let services = service(&root, "options", Appearance::Dark).unwrap();
        let probe = RenderProbe::default();
        let mut dom = VirtualDom::new(App);
        dom.provide_root_context(services.clone());
        dom.provide_root_context(probe.clone());
        dom.rebuild_in_place();
        let initial = services.ui_resolutions.load(Ordering::Relaxed);
        let mut handles = probe.0.borrow().expect("actual App signal handles");
        for _ in 0..100 {
            dom.mark_dirty(handles.scope);
            dom.render_immediate_to_vec();
        }
        let after_unrelated_renders = services.ui_resolutions.load(Ordering::Relaxed);
        dom.in_scope(ScopeId::ROOT, || {
            set_value(
                &mut handles.document.write(),
                "notifications.playSound",
                Some(json!(false)),
            );
        });
        dom.render_immediate_to_vec();
        let after_edit = services.ui_resolutions.load(Ordering::Relaxed);
        dom.in_scope(ScopeId::ROOT, || handles.appearance.set(Appearance::Light));
        dom.render_immediate_to_vec();
        let after_appearance = services.ui_resolutions.load(Ordering::Relaxed);
        dom.in_scope(ScopeId::ROOT, || handles.document.set(json!([])));
        dom.render_immediate_to_vec();
        let after_invalid = services.ui_resolutions.load(Ordering::Relaxed);
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("Configuration must be a JSON object"));
        let schemas = UI_SCHEMA_PARSES.with(std::cell::Cell::get);
        eprintln!(
            "UI work: initial={initial}, after100={after_unrelated_renders}, edit={after_edit}, appearance={after_appearance}, invalid={after_invalid}, schema_parses={schemas}"
        );
        assert_eq!(
            initial, 1,
            "All inherited field values share one resolution"
        );
        assert_eq!(
            after_unrelated_renders, 1,
            "Unrelated renders must not resolve settings again"
        );
        assert_eq!(after_edit, 2, "A document edit must resolve exactly once");
        assert_eq!(
            after_appearance, 2,
            "Appearance must not invalidate imaging derivations"
        );
        assert_eq!(
            after_invalid, 3,
            "Invalid input must be recomputed and shown, not cached away"
        );
        assert_eq!(
            schemas, 1,
            "Schema descriptors are immutable for the service lifetime"
        );
    }

    #[test]
    fn boot_editor_derivations_follow_edits_not_unrelated_renders_and_report_invalid_input() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let services = service(&root, "boot-editor", Appearance::Dark).unwrap();
        let probe = BootProbe::default();
        UI_BOOT_DERIVATIONS.with(|count| count.set(0));
        let mut dom = VirtualDom::new(App);
        dom.provide_root_context(services.clone());
        dom.provide_root_context(probe.clone());
        dom.rebuild_in_place();
        let mut handles = probe.0.borrow().expect("actual protected editor handles");
        assert_eq!(services.ui_resolutions.load(Ordering::Relaxed), 1);
        assert_eq!(UI_BOOT_DERIVATIONS.with(std::cell::Cell::get), 1);
        for _ in 0..20 {
            dom.mark_dirty(handles.scope);
            dom.render_immediate_to_vec();
        }
        dom.in_scope(handles.scope, || handles.advanced.set(true));
        dom.render_immediate_to_vec();
        assert_eq!(UI_BOOT_DERIVATIONS.with(std::cell::Cell::get), 1);
        let current = handles.computed.read().as_ref().unwrap()["fields"]
            .as_array()
            .unwrap()
            .iter()
            .find(|field| field["key"] == "enable_uart")
            .unwrap()["value"]
            .clone();
        let changed = if current == "1" { "0" } else { "1" };
        dom.in_scope(handles.scope, || {
            handles.context.write()["values"]["enable_uart"] = json!(changed)
        });
        dom.render_immediate_to_vec();
        assert_eq!(UI_BOOT_DERIVATIONS.with(std::cell::Cell::get), 2);
        assert!(
            handles.computed.read().as_ref().unwrap()["fields"]
                .as_array()
                .unwrap()
                .iter()
                .any(|field| field["key"] == "enable_uart" && field["value"] == changed)
        );
        dom.in_scope(handles.scope, || handles.context.set(json!({})));
        dom.render_immediate_to_vec();
        assert_eq!(UI_BOOT_DERIVATIONS.with(std::cell::Cell::get), 3);
        assert!(handles.computed.read().is_err());
        let html = dioxus_ssr::render(&dom);
        let apply = html
            .find("Apply validated settings")
            .expect("actual Apply control");
        let button_start = html[..apply]
            .rfind("<button")
            .expect("Apply button opening");
        let button = html[button_start..apply]
            .split_once('>')
            .expect("Apply button attributes")
            .0;
        assert!(
            button.contains(" disabled"),
            "Invalid settings must disable the actual Apply button"
        );
    }
}
