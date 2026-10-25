use anyhow::{Context, anyhow};
use clap::Parser;
use dioxus::desktop::tao::event::{Event as NativeEvent, WindowEvent};
use dioxus::desktop::tao::{dpi::LogicalSize, window::Theme as WindowTheme};
use dioxus::desktop::{Config, WindowBuilder, WindowCloseBehaviour};
use dioxus::prelude::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
use wor_application::wizard::{
    FormField, Page, applicable, schema_fields, set_value, switch_family, title,
};
use wor_application::{EngineEvent, Operation, Preview, Request, Selection};
use wor_domain::configuration::{NativeConfig, SETTINGS, at_path};
use wor_domain::progress::Progress;
use wor_infrastructure::device::DeviceSnapshot;
use wor_infrastructure::{
    Engine, EngineError, OutputMode, Runtime, bundled_product, config_digest, interruption,
};

mod appearance;
#[cfg(feature = "visual-fixtures")]
mod visual_fixtures;
use appearance::Appearance;

#[derive(Parser)]
#[command(about = "Guided native Rust imaging workflow for WoR-Flasher")]
struct Options {
    #[arg(long)]
    runtime_dir: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    check_runtime: bool,
    #[arg(
        long,
        help = "Mount and close a local window without engine operations"
    )]
    smoke_test: bool,
    #[arg(long, requires = "smoke_test", value_parser = clap::value_parser!(u16).range(320..=1920))]
    smoke_width: Option<u16>,
    #[arg(long, help = "Override the initial System/Light/Dark appearance")]
    appearance: Option<Appearance>,
    #[cfg(feature = "visual-fixtures")]
    #[arg(long, hide = true)]
    export_ui_fixtures: Option<PathBuf>,
}

enum Action {
    Inventory,
    Catalog(String),
    Preview {
        selection: Selection,
    },
    Run {
        selection: Selection,
        preview: Preview,
        confirmed: bool,
        wipe: bool,
        native: bool,
        password: String,
    },
    Account {
        arguments: Vec<std::ffi::OsString>,
        input: Value,
        configure: bool,
    },
    Close,
}

struct Services {
    engine: Option<Engine>,
    root: PathBuf,
    metadata: Value,
    host_locale: String,
    fields: Vec<Arc<FormField>>,
    initial: Value,
    presentation: Presentation,
    appearance_path: Option<PathBuf>,
    visual_fixture: bool,
    draft: tempfile::TempDir,
    sender: mpsc::UnboundedSender<Action>,
    receiver: Mutex<Option<mpsc::UnboundedReceiver<Action>>>,
    cancellation: Mutex<Option<watch::Sender<bool>>>,
    busy: AtomicBool,
    closing: AtomicBool,
    smoke: bool,
    alert_auto: AtomicBool,
    alert_monitor: Mutex<Option<wor_infrastructure::disk_alerts::Monitor>>,
    alert_warning: Arc<Mutex<Option<String>>>,
    #[cfg(test)]
    ui_resolutions: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
std::thread_local! {
    static UI_SCHEMA_PARSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static UI_BOOT_DERIVATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

type UiConfig = std::result::Result<NativeConfig, String>;

fn prepare_fields(schema: Value) -> Vec<Arc<FormField>> {
    #[cfg(test)]
    UI_SCHEMA_PARSES.with(|count| count.set(count.get() + 1));
    schema_fields(&schema).into_iter().map(Arc::new).collect()
}

impl Services {
    fn send(&self, action: Action) {
        if self.visual_fixture || self.smoke {
            tracing::error!("Visual fixtures and smoke tests cannot start native operations");
            return;
        }
        if !matches!(action, Action::Close)
            && self
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            tracing::warn!("An operation is already queued or active");
            return;
        }
        if self.sender.send(action).is_err() {
            self.busy.store(false, Ordering::Release);
            tracing::error!("Native controller unavailable");
        }
    }
    fn cancel(&self) {
        if let Err(error) = self.stop_alerts() {
            tracing::warn!(error=%error,"Disk-alert cleanup failed");
        }
        match self.cancellation.lock() {
            Ok(value) => {
                if let Some(sender) = value.as_ref()
                    && sender.send(true).is_err()
                {
                    tracing::warn!("The operation already stopped");
                }
            }
            Err(error) => tracing::error!(error=%error,"Cannot access cancellation state"),
        }
    }
    fn start_alerts(&self) -> Result<(), String> {
        if !self.alert_auto.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut monitor = self
            .alert_monitor
            .lock()
            .map_err(|_| "Disk-alert state lock failed")?;
        if monitor.is_none() {
            let state = self.alert_warning.clone();
            *monitor = Some(wor_infrastructure::disk_alerts::Monitor::start(Arc::new(
                move |message| match state.lock() {
                    Ok(mut warning) => *warning = Some(message),
                    Err(error) => {
                        tracing::error!(error=%error,"Disk-alert warning unavailable")
                    }
                },
            ))?);
        }
        Ok(())
    }
    fn stop_alerts(&self) -> Result<(), String> {
        if let Some(mut monitor) = self
            .alert_monitor
            .lock()
            .map_err(|_| "Disk-alert state lock failed")?
            .take()
        {
            monitor.stop()?;
        }
        Ok(())
    }
    fn resolved(&self, document: &Value) -> anyhow::Result<NativeConfig> {
        #[cfg(test)]
        self.ui_resolutions.fetch_add(1, Ordering::Relaxed);
        let home = std::env::var("HOME").context("HOME is required")?;
        Ok(NativeConfig::resolve(
            document,
            &BTreeMap::new(),
            &self.metadata,
            &home,
            &self.host_locale,
            cfg!(target_os = "macos"),
        )?)
    }
    fn snapshot(&self, document: &Value) -> anyhow::Result<PathBuf> {
        let mut file = tempfile::Builder::new()
            .prefix("run-")
            .suffix(".json")
            .tempfile_in(self.draft.path())?;
        serde_json::to_writer(&mut file, document)?;
        use std::io::Write;
        file.flush()?;
        let (_, path) = file.keep().map_err(|error| error.error)?;
        Ok(path)
    }
}

#[derive(Clone, Default)]
struct View {
    busy: bool,
    blocked: bool,
    error: String,
    status: String,
    details: String,
    percent: f64,
    step: String,
    task: String,
    devices: Vec<DeviceSnapshot>,
    catalog: Value,
    account_probe: Option<Value>,
    account_result: Option<Value>,
}

#[derive(Clone)]
struct Presentation {
    page: Page,
    view: View,
    preview: Option<Preview>,
    boot_editor: bool,
    advanced_editor: bool,
    appearance: Appearance,
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            page: Page::Image,
            view: View::default(),
            preview: None,
            boot_editor: false,
            advanced_editor: false,
            appearance: Appearance::System,
        }
    }
}

const STYLE: &str = include_str!("styles.css");

fn partnership_asset_response(
    method: &str,
    path: &str,
) -> dioxus::desktop::wry::http::Response<Vec<u8>> {
    let (status, content_type, body): (u16, &str, &[u8]) = match (method, path) {
        ("GET" | "HEAD", "/assets/partnership.png") => (
            200,
            "image/png",
            include_bytes!("../../../assets/partnership.png"),
        ),
        ("GET" | "HEAD", _) => (404, "text/plain", b"Unknown partnership asset"),
        _ => (405, "text/plain", b"Read-only partnership asset"),
    };
    dioxus::desktop::wry::http::Response::builder()
        .status(status)
        .header("Content-Type", content_type)
        .header("X-Content-Type-Options", "nosniff")
        .header("Cache-Control", "no-store")
        .header("Allow", "GET, HEAD")
        .body(if method == "HEAD" {
            Vec::new()
        } else {
            body.to_vec()
        })
        .expect("Static partnership response is valid")
}

#[cfg(test)]
mod theme_tests {
    use super::*;

    #[test]
    fn partnership_route_serves_only_exact_local_artwork_and_never_writes() {
        let response = partnership_asset_response("GET", "/assets/partnership.png");
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["Content-Type"], "image/png");
        assert_eq!(response.headers()["X-Content-Type-Options"], "nosniff");
        assert_eq!(
            response.body(),
            include_bytes!("../../../assets/partnership.png")
        );
        let head = partnership_asset_response("HEAD", "/assets/partnership.png");
        assert_eq!(head.status(), 200);
        assert!(head.body().is_empty());
        for path in ["/assets/../LICENSE", "/assets/other.png", "/etc/passwd"] {
            assert_eq!(partnership_asset_response("GET", path).status(), 404);
            let head = partnership_asset_response("HEAD", path);
            assert_eq!(head.status(), 404);
            assert!(head.body().is_empty());
        }
        for method in ["POST", "PUT", "DELETE"] {
            assert_eq!(
                partnership_asset_response(method, "/assets/partnership.png").status(),
                405
            );
            assert_eq!(
                partnership_asset_response(method, "/assets/partnership.png").headers()["Allow"],
                "GET, HEAD"
            );
        }
    }
}

fn window_theme(appearance: Appearance) -> Option<WindowTheme> {
    match appearance {
        Appearance::System => None,
        Appearance::Light => Some(WindowTheme::Light),
        Appearance::Dark => Some(WindowTheme::Dark),
    }
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("warn"))
        .init();
    let options = Options::parse();
    let executable = std::env::current_exe()?;
    let runtime = Runtime::resolve(options.runtime_dir.as_deref(), &executable)?;
    if options.check_runtime {
        println!("Native runtime is complete and valid.");
        return Ok(());
    }
    let root = runtime.root().to_owned();
    #[cfg(feature = "visual-fixtures")]
    if let Some(output) = options.export_ui_fixtures {
        return visual_fixtures::export(&root, &output);
    }
    let path = options
        .config
        .unwrap_or_else(|| root.join("config-templates/config.json"));
    config_digest(&path)?;
    let mut initial: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    set_value(&mut initial, "execution.dryRun", Some(json!(true)));
    set_value(
        &mut initial,
        "execution.confirmIotErase",
        Some(json!(false)),
    );
    set_value(&mut initial, "execution.confirmIotWipe", Some(json!(false)));
    let metadata = serde_json::from_slice(&std::fs::read(root.join("src/config/metadata.json"))?)?;
    let schema = serde_json::from_slice(&std::fs::read(
        root.join("config-templates/config.schema.json"),
    )?)?;
    let helper = executable
        .parent()
        .context("CLI companion directory")?
        .join("wor-flasher");
    let engine = if options.smoke_test {
        None
    } else {
        Some(Engine::new(runtime, helper)?)
    };
    let home = std::env::var_os("HOME").context("HOME is required")?;
    let appearance_path = appearance::preference_path(Path::new(&home), cfg!(target_os = "macos"));
    let mut presentation = Presentation::default();
    match options
        .appearance
        .map_or_else(|| appearance::load(&appearance_path), Ok)
    {
        Ok(mode) => presentation.appearance = mode,
        Err(error) => {
            tracing::warn!(error = %error, "Saved appearance could not be loaded");
            presentation.view.error = format!(
                "Saved appearance could not be loaded; using System until you choose a mode: {error}"
            );
        }
    }
    let (sender, receiver) = mpsc::unbounded_channel();
    let services = Arc::new(Services {
        engine,
        root,
        metadata,
        host_locale: wor_infrastructure::desktop_system::host_locale(),
        fields: prepare_fields(schema),
        initial,
        presentation,
        appearance_path: (!options.smoke_test).then_some(appearance_path),
        visual_fixture: false,
        draft: tempfile::tempdir()?,
        sender,
        receiver: Mutex::new(Some(receiver)),
        cancellation: Mutex::new(None),
        busy: AtomicBool::new(false),
        closing: AtomicBool::new(false),
        smoke: options.smoke_test,
        alert_auto: AtomicBool::new(wor_infrastructure::disk_alerts::accessibility_trusted()),
        alert_monitor: Mutex::new(None),
        alert_warning: Arc::new(Mutex::new(None)),
        #[cfg(test)]
        ui_resolutions: std::sync::atomic::AtomicUsize::new(0),
    });
    let events = services.clone();
    let product = bundled_product()?;
    let config = Config::new()
        .with_resource_directory(&services.root)
        .with_window(
            WindowBuilder::new()
                .with_inner_size(LogicalSize::new(
                    f64::from(options.smoke_width.unwrap_or(1100)),
                    780.0,
                ))
                .with_min_inner_size(LogicalSize::new(320.0, 480.0))
                .with_theme(window_theme(services.presentation.appearance))
                .with_title(format!(
                    "{} {} - Native Rust",
                    product.name, product.version
                )),
        )
        .with_close_behaviour(WindowCloseBehaviour::WindowHides)
        .with_menu(None)
        .with_disable_context_menu(true)
        .with_disable_drag_drop_handler(true)
        .with_navigation_handler(|_| false)
        .with_custom_event_handler(move |event, _| {
            if matches!(
                event,
                NativeEvent::WindowEvent {
                    event: WindowEvent::CloseRequested,
                    ..
                }
            ) {
                events.closing.store(true, Ordering::Release);
                events.cancel();
                events.send(Action::Close);
            }
        });
    dioxus::LaunchBuilder::desktop()
        .with_context(services)
        .with_cfg(config)
        .launch(App);
    Ok(())
}

fn change(
    mut document: Signal<Value>,
    mut revision: Signal<u64>,
    path: &str,
    value: Option<Value>,
) {
    if path == "media.sourceFile"
        && value
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
    {
        set_value(
            &mut document.write(),
            "media.downloadIotImage",
            Some(json!(false)),
        );
    }
    if path == "media.downloadIotImage" && value == Some(json!(true)) {
        set_value(&mut document.write(), "media.sourceFile", Some(json!("")));
    }
    set_value(&mut document.write(), path, value);
    revision += 1;
}

fn text(document: &Value, path: &str) -> String {
    at_path(document, path).map_or_else(String::new, |value| {
        value
            .as_str()
            .map_or_else(|| value.to_string(), str::to_owned)
    })
}

async fn observed(
    engine: &Engine,
    request: Request,
    cancellation: watch::Receiver<bool>,
    mut update: impl FnMut(EngineEvent),
) -> Result<wor_application::Outcome, EngineError> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let sink = Arc::new(move |event| {
        if sender.send(event).is_err() {
            tracing::warn!("Native progress viewer closed");
        }
    });
    let execute = engine.execute(request, OutputMode::Capture, cancellation, sink);
    tokio::pin!(execute);
    loop {
        tokio::select! {result=&mut execute=>return result,Some(event)=receiver.recv()=>update(event)}
    }
}

#[component]
fn Field(
    field: Arc<FormField>,
    document: Signal<Value>,
    revision: Signal<u64>,
    disabled: bool,
    normalized: Memo<UiConfig>,
) -> Element {
    let explicit = at_path(&document.read(), &field.path).is_some();
    let value = if explicit || field.path == "customization.uefiUseLatest" {
        text(&document.read(), &field.path)
    } else {
        normalized
            .read()
            .as_ref()
            .ok()
            .and_then(|config| {
                SETTINGS
                    .iter()
                    .find(|spec| spec.paths.first().is_some_and(|path| *path == field.path))
                    .map(|spec| config.get(spec.name).to_owned())
            })
            .unwrap_or_default()
    };
    let path = field.path.clone();
    let path2 = path.clone();
    let mut error = use_signal(String::new);
    let title = title(&path);
    let integer = field.kind == "integer";
    let nullable = field.nullable;
    let id = format!("setting-{}", field.path);
    let description = (!field.description.is_empty()).then(|| format!("{id}-help"));
    rsx! {div {class:"field",
        if field.kind=="boolean" {
            if field.path=="customization.uefiUseLatest" {
                label {r#for:"{id}","{title}"}
                select {id:"{id}",aria_describedby:description.clone(),disabled,value:"{value}",onchange:move|event|{
                    let value=match event.value().as_str(){"true"=>Some(json!(true)),"false"=>Some(json!(false)),_=>None};
                    change(document,revision,&path,value);
                },
                    option {value:"","Use the selected model default"}
                    option {value:"true","Latest stable"}
                    option {value:"false","Selected/pinned version"}
                }
            } else {
                label {r#for:"{id}",input {id:"{id}",aria_describedby:description.clone(),r#type:"checkbox",disabled,checked:value=="true"||value=="1",onchange:move|event|change(document,revision,&path,Some(json!(event.checked())))} "{title}"}
            }
        } else if field.path=="media.winLang" {
            label {r#for:"{id}","Windows image language"}
            select {id:"{id}",aria_describedby:description.clone(),disabled,value:"{value}",onchange:move|event|change(document,revision,&path,Some(json!(event.value()))),
                for (code,label) in wor_domain::configuration::LANGUAGES {option {value:"{code}","{label} ({code})"}}
            }
        } else if !field.options.is_empty() {
            label {r#for:"{id}","{title}"}
            select {id:"{id}",aria_describedby:description.clone(),disabled,value:"{value}",onchange:move|event|{
                let value=event.value();
                let parsed=if integer {value.parse::<i64>().ok().map(|value|json!(value))} else {Some(json!(value))};
                if let Some(value)=parsed {change(document,revision,&path,Some(value));}
            },
                option {value:"","Inherit default"}
                for option in &field.options {
                    option {value:"{option.as_str().map_or_else(||option.to_string(),str::to_owned)}","{option.as_str().map_or_else(||option.to_string(),str::to_owned)}"}
                }
            }
        } else if field.kind=="integer" || field.kind=="number" {
            label {r#for:"{id}","{title}"}
            input {id:"{id}",aria_describedby:description.clone(),r#type:"text",inputmode:"numeric",disabled,value:"{value}",oninput:move|event|{
                let value=event.value();
                if value.is_empty() {change(document,revision,&path,if nullable {Some(Value::Null)}else{None});error.set(String::new());}
                else {match value.parse::<i64>() {Ok(value)=>{change(document,revision,&path,Some(json!(value)));error.set(String::new());},Err(_)=>error.set("Enter a valid integer; no preference was silently changed.".into())}}
            }}
        } else {
            label {r#for:"{id}","{title}"}
            input {id:"{id}",aria_describedby:description.clone(),r#type:if field.secret {"password"}else{"text"},disabled,value:"{value}",autocomplete:"off",
                oninput:move|event|change(document,revision,&path,Some(json!(event.value())))}
        }
        if !field.description.is_empty(){small {id:"{id}-help","{field.description}"}}
        if !error().is_empty(){p {class:"error","{error}"}}
        if explicit{button {disabled,onclick:move |_|change(document,revision,&path2,None),"Use default"}}
    }}
}

fn boot_context(
    root: &Path,
    model: &str,
    profile: &str,
    config: &str,
    mode: &str,
) -> anyhow::Result<Value> {
    if profile == "iot-core" {
        let preset = if mode == "1080p60" {
            "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=16\n"
        } else {
            "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n"
        };
        return Ok(
            json!({"profile":"iot-core","config":if config.is_empty(){preset}else{config},"hdmiMode":"custom"}),
        );
    }
    if !matches!(model, "3" | "4" | "5") {
        return Err(anyhow!(
            "Choose a supported model before editing boot configuration"
        ));
    }
    let official =
        std::fs::read_to_string(root.join(format!("config-templates/pi{model}.config.txt")))?;
    let recommended = if model == "4" {
        format!("{official}\nhdmi_force_hotplug=1\nhdmi_group=2\nhdmi_mode=69\nhdmi_drive=2\n")
    } else {
        official.clone()
    };
    let choices = if model == "4" {
        json!([{"value":"official","label":"Official image default"},{"value":"recommended","label":"Recommended (1920 x 1200 / 60 Hz)"},{"value":"custom","label":"Custom"}])
    } else {
        json!([{"value":"official","label":"Official image default"},{"value":"custom","label":"Custom"}])
    };
    Ok(
        json!({"profile":"desktop","model":model,"official":official,"recommended":recommended,"hdmiChoices":choices,
        "config":if config.is_empty(){if model=="4"{recommended}else{official}}else{config.to_owned()},"hdmiMode":mode}),
    )
}

fn change_board(
    services: &Services,
    mut document: Signal<Value>,
    mut revision: Signal<u64>,
    value: &str,
    iot: bool,
) -> anyhow::Result<()> {
    if iot {
        change(document, revision, "target.board", Some(json!(value)));
        return Ok(());
    }
    let model = if value == "pi2-v1.2" { "3" } else { value };
    let old_model = text(&document.read(), "target.rpiModel");
    let existing = text(&document.read(), "customization.configTxt");
    let mode = text(&document.read(), "customization.hdmiMode");
    let mut next = document();
    if !existing.is_empty() && old_model != model {
        let previous = boot_context(&services.root, &old_model, "desktop", &existing, &mode)?;
        let mode = if mode.is_empty() {
            if model == "4" {
                "recommended"
            } else {
                "official"
            }
        } else if model != "4" && mode == "recommended" {
            "official"
        } else {
            mode.as_str()
        };
        let mut request = boot_context(&services.root, model, "desktop", &existing, mode)?;
        request["previousDefaults"] = previous;
        let result = wor_domain::boot_config::process(&request)?;
        set_value(
            &mut next,
            "customization.configTxt",
            Some(result["config"].clone()),
        );
        set_value(&mut next, "customization.hdmiMode", Some(json!(mode)));
    }
    let number = model
        .parse::<u8>()
        .context("Invalid physical board route")?;
    set_value(&mut next, "target.rpiModel", Some(json!(number)));
    set_value(
        &mut next,
        "target.board",
        if value == "pi2-v1.2" {
            Some(json!("pi2-v1.2"))
        } else {
            None
        },
    );
    document.set(next);
    revision += 1;
    Ok(())
}

#[component]
fn BootEditor(
    document: Signal<Value>,
    revision: Signal<u64>,
    mut open: Signal<bool>,
    normalized: Memo<UiConfig>,
) -> Element {
    let services = use_context::<Arc<Services>>();
    let mut context = use_signal(|| {
        let config = normalized.read();
        let initial = config
            .as_ref()
            .map_err(|error| anyhow!("{error}"))
            .and_then(|config| {
                let iot = config.get("WOR_IMAGE_FAMILY") == "iot-core";
                boot_context(
                    &services.root,
                    config.get("RPI_MODEL"),
                    if iot { "iot-core" } else { "desktop" },
                    config.get(if iot {
                        "IOT_CORE_HDMI_CONFIG"
                    } else {
                        "CONFIG_TXT"
                    }),
                    if iot {
                        "custom"
                    } else {
                        if config.get("HDMI_MODE").is_empty() {
                            if config.get("RPI_MODEL") == "4" {
                                "recommended"
                            } else {
                                "official"
                            }
                        } else {
                            config.get("HDMI_MODE")
                        }
                    },
                )
            });
        initial.unwrap_or_else(|error| json!({"error":error.to_string()}))
    });
    let mut advanced = use_signal(|| services.presentation.advanced_editor);
    let computed = use_memo(move || {
        #[cfg(test)]
        UI_BOOT_DERIVATIONS.with(|count| count.set(count.get() + 1));
        wor_domain::boot_config::process(&context.read()).map_err(|error| error.to_string())
    });
    #[cfg(all(test, feature = "visual-fixtures"))]
    if let Some(probe) = try_consume_context::<visual_fixtures::BootProbe>() {
        *probe.0.borrow_mut() = Some(visual_fixtures::BootHandles {
            context,
            advanced,
            computed,
            scope: dioxus::dioxus_core::current_scope_id(),
        });
    }
    let state = computed.read();
    let error = state
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_else(|| text(&context.read(), "error"));
    let fields = state
        .as_ref()
        .ok()
        .and_then(|state| state.get("fields"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let warnings = state
        .as_ref()
        .ok()
        .and_then(|state| state.get("warnings"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    rsx! {section {class:"card",role:"region",aria_labelledby:"boot-editor-title",
        h2 {id:"boot-editor-title","Protected Boot Config Editor"}
        p {class:"muted","Required firmware entries remain protected. Guided settings and Advanced Text Edit use the same native validator."}
        for field in fields {
            label {
                {field["label"].as_str().unwrap_or("Boot setting")}
                if field["type"]=="boolean" {
                    {let key=field["key"].as_str().unwrap_or("").to_owned();rsx!{input {r#type:"checkbox",disabled:field["disabled"].as_bool().unwrap_or(false),
                        checked:field["value"]=="1",onchange:move|event|context.write()["values"][&key]=json!(if event.checked(){"1"}else{"0"})}}}
                } else {
                    {let key=field["key"].as_str().unwrap_or("").to_owned();rsx!{input {r#type:"text",disabled:field["disabled"].as_bool().unwrap_or(false),
                        value:field["value"].as_str().unwrap_or("").to_owned(),oninput:move|event|context.write()["values"][&key]=json!(event.value())}}}
                }
            }
        }
        label {input {r#type:"checkbox",checked:advanced(),onchange:move|event|advanced.set(event.checked())}"Advanced Text Edit (Not Recommended)"}
        if advanced() {textarea {aria_label:"Advanced boot configuration",value:text(&context.read(),"config"),oninput:move|event|{
            context.write()["config"]=json!(event.value());context.write()["values"]=json!({});
        }}}
        for warning in warnings {p {class:"warning",{warning.as_str().unwrap_or("Boot settings require review")}}}
        if !error.is_empty(){p {class:"error","{error}"}}
        div {class:"actions",
            button {onclick:move |_|{
                let defaults=if context.read()["profile"]=="iot-core" {"hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n".to_owned()} else {
                    let current=context.read();
                    current["recommended"].as_str().or_else(||current["official"].as_str()).unwrap_or("").to_owned()
                };
                context.write()["config"]=json!(defaults);context.write()["values"]=json!({});
            },"Restore model defaults"}
            button {class:"primary",disabled:state.is_err(),onclick:move |_|{
                if let Ok(state)=wor_domain::boot_config::process(&context.read()) {
                    let iot=context.read()["profile"]=="iot-core";
                    change(document,revision,if iot{"customization.iotHdmiConfig"}else{"customization.configTxt"},Some(state["config"].clone()));
                    change(document,revision,if iot{"customization.iotHdmiMode"}else{"customization.hdmiMode"},Some(json!("custom")));
                    if !iot {change(document,revision,"customization.bootSettings",None);}
                    open.set(false);
                }
            },"Apply validated settings"}
            button {onclick:move |_|open.set(false),"Cancel"}
        }
    }}
}

#[component]
fn App() -> Element {
    let services = use_context::<Arc<Services>>();
    let mut document = use_signal(|| services.initial.clone());
    let derivation_services = services.clone();
    let normalized = use_memo(move || {
        derivation_services
            .resolved(&document.read())
            .map_err(|error| error.to_string())
    });
    let mut revision = use_signal(|| 0_u64);
    let mut page = use_signal(|| services.presentation.page);
    let mut view = use_signal(|| services.presentation.view.clone());
    let mut preview = use_signal(|| services.presentation.preview.clone());
    let mut erase = use_signal(|| false);
    let mut wipe = use_signal(|| false);
    let mut native = use_signal(|| false);
    let mut password = use_signal(String::new);
    let mut boot_editor = use_signal(|| services.presentation.boot_editor);
    let mut appearance = use_signal(|| services.presentation.appearance);
    #[cfg(all(test, feature = "visual-fixtures"))]
    if let Some(probe) = try_consume_context::<visual_fixtures::RenderProbe>() {
        *probe.0.borrow_mut() = Some(visual_fixtures::RenderHandles {
            document,
            appearance,
            scope: dioxus::dioxus_core::current_scope_id(),
        });
    }
    let mut options_original = use_signal(|| None::<Value>);
    let mut fingerprint_confirmed = use_signal(|| false);
    let mut accessibility = use_signal(|| {
        if services.visual_fixture || !cfg!(target_os = "macos") {
            0_i8
        } else if wor_infrastructure::disk_alerts::accessibility_trusted() {
            1
        } else {
            -1
        }
    });
    let interactive = !services.visual_fixture;
    let window = if interactive {
        dioxus::desktop::use_asset_handler("assets", |request, responder| {
            responder.respond(partnership_asset_response(
                request.method().as_str(),
                request.uri().path(),
            ));
        });
        Some(dioxus::desktop::use_window())
    } else {
        None
    };
    let appearance_window = window.clone();
    use_effect(move || {
        let mode = appearance();
        if let Some(window) = &appearance_window {
            window.set_theme(window_theme(mode));
        }
    });
    use_effect(move || {
        if !interactive {
            return;
        }
        let _ = revision();
        preview.set(None);
        erase.set(false);
        wipe.set(false);
        native.set(false);
        fingerprint_confirmed.set(false);
        view.write().account_probe = None;
        view.write().account_result = None;
    });
    let signals = services.clone();
    use_future(move || {
        let services = signals.clone();
        async move {
            if !interactive {
                return;
            }
            let interruption = interruption();
            match interruption {
                Ok(interruption) => {
                    if let Err(error) = interruption.await {
                        tracing::error!(error=%error,"Signal observer failed");
                    }
                }
                Err(error) => tracing::error!(error=%error,"Cannot register signal observers"),
            }
            services.closing.store(true, Ordering::Release);
            services.cancel();
            services.send(Action::Close);
        }
    });
    let controller = services.clone();
    let controlled_window = window.clone();
    let warnings = services.clone();
    use_future(move || {
        let services = warnings.clone();
        async move {
            if !interactive {
                return;
            }
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                let warning = match services.alert_warning.lock() {
                    Ok(mut warning) => warning.take(),
                    Err(error) => Some(format!("Disk-alert diagnostics failed: {error}")),
                };
                if let Some(warning) = warning {
                    view.write().error = warning;
                }
            }
        }
    });
    use_future(move || {
        let services = controller.clone();
        let window = controlled_window.clone();
        async move {
            if !interactive {
                return;
            }
            let Some(window) = window else {
                view.write().blocked = true;
                view.write().error =
                    "Native window context is unavailable; no operation can start.".into();
                tracing::error!("Native window context unavailable");
                return;
            };
            let mut receiver = services
                .receiver
                .lock()
                .expect("controller mutex")
                .take()
                .expect("single controller");
            while let Some(action) = receiver.recv().await {
                if matches!(action, Action::Close) {
                    if view.peek().blocked {
                        window.set_visible(true);
                    } else {
                        window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
                        window.close();
                    }
                    continue;
                }
                view.write().busy = true;
                view.write().error.clear();
                view.write().details.clear();
                let result: anyhow::Result<()> = async {
                    let (cancel, cancellation) =
                        watch::channel(services.closing.load(Ordering::Acquire));
                    *services
                        .cancellation
                        .lock()
                        .map_err(|_| anyhow!("Cancellation mutex failed"))? = Some(cancel.clone());
                    if services.closing.load(Ordering::Acquire) {
                        cancel
                            .send(true)
                            .map_err(|_| anyhow!("Cancellation closed"))?;
                    }
                    let mut selected = None;
                    let mut account = false;
                    let mut account_configure = false;
                    let mut catalogue_kind = None;
                    let (mut request, kind) = match action {
                        Action::Inventory => {
                            (Request::new(Operation::DeviceInventory), "inventory")
                        }
                        Action::Catalog(kind) => {
                            catalogue_kind = Some(kind.clone());
                            (Request::new(Operation::Catalog(kind)), "catalog")
                        }
                        Action::Preview { selection } => {
                            selection.validate()?;
                            selected = Some(selection);
                            (Request::new(Operation::Preview), "preview")
                        }
                        Action::Run {
                            selection,
                            preview: approved,
                            confirmed,
                            wipe,
                            native,
                            password: credential,
                        } => {
                            approved.authorize(
                                &selection,
                                &config_digest(&selection.config)?,
                                confirmed,
                            )?;
                            let mut request = Request::new(Operation::Run(Vec::new()));
                            request.target_fingerprint = approved.target_fingerprint;
                            request.allow_native_write = native;
                            if !credential.is_empty() {
                                request.sudo_password = Some(credential);
                            }
                            request.settings.push(
                                format!("WOR_IOT_CONFIRM_ERASE={}", u8::from(confirmed)).parse()?,
                            );
                            request
                                .settings
                                .push(format!("WOR_IOT_CONFIRM_WIPE={}", u8::from(wipe)).parse()?);
                            selected = Some(selection);
                            page.set(Page::Progress);
                            (request, "run")
                        }
                        Action::Account {
                            arguments,
                            input,
                            configure,
                        } => {
                            let mut request = Request::new(Operation::IotAccount(arguments));
                            request.private_input = Some(serde_json::to_string(&input)?);
                            account = true;
                            account_configure = configure;
                            (request, "account")
                        }
                        Action::Close => unreachable!(),
                    };
                    request.graphical = true;
                    if let Some(selected) = &selected {
                        request.config = Some(selected.config.clone());
                        request.expected_config_digest = Some(config_digest(&selected.config)?);
                        request
                            .settings
                            .push(format!("DEVICE={}", selected.device.display()).parse()?);
                        request
                            .settings
                            .push(format!("DRY_RUN={}", u8::from(selected.dry_run)).parse()?);
                    } else {
                        request.config = Some(services.snapshot(&document.peek())?);
                    }
                    let outcome = observed(
                        services.engine.as_ref().context(
                            "Native engine unavailable; visual fixtures cannot execute operations",
                        )?,
                        request,
                        cancellation,
                        |event| match event {
                            EngineEvent::Progress(Progress::Status(message)) => {
                                view.write().status = message
                            }
                            EngineEvent::Progress(Progress::Step {
                                current,
                                total,
                                message,
                            }) => {
                                view.write().step = format!("{current}/{total} - {message}");
                                view.write().percent = 0.0;
                            }
                            EngineEvent::Progress(Progress::Substep(value)) => {
                                view.write().percent = value
                            }
                            EngineEvent::Progress(Progress::Task { percent, label }) => {
                                view.write().percent = percent;
                                view.write().task = label;
                            }
                            EngineEvent::Progress(Progress::DiskWrite { active, .. }) => {
                                let result = if active {
                                    services.start_alerts()
                                } else {
                                    services.stop_alerts()
                                };
                                if let Err(error) = result {
                                    view.write().error = error;
                                }
                            }
                            EngineEvent::Diagnostic(message) => view.write().error = message,
                        },
                    )
                    .await
                    .map_err(|error| {
                        if matches!(error, EngineError::Cleanup(_)) {
                            view.write().blocked = true;
                        }
                        error
                    })?;
                    view.write().status =
                        outcome.label(selected.as_ref().is_some_and(|value| value.dry_run));
                    if kind == "run" && !outcome.cancelled {
                        let config = services.resolved(&document.peek())?;
                        let success = outcome.succeeded();
                        let warnings = tokio::task::spawn_blocking(move || {
                            wor_infrastructure::desktop_system::completion_feedback(
                                &config, success,
                            )
                        })
                        .await?;
                        if !warnings.is_empty() {
                            view.write().error = warnings.join("\n");
                        }
                    }
                    if outcome.exit_code == 74 {
                        view.write().blocked = true;
                    }
                    if !outcome.succeeded() {
                        view.write().details = format!("{}{}", outcome.stdout, outcome.stderr);
                        if kind == "run" {
                            page.set(Page::Result);
                        }
                        return Err(anyhow!(
                            "{}\n{}",
                            outcome.label(selected.as_ref().is_some_and(|value| value.dry_run)),
                            outcome.stderr
                        ));
                    }
                    let value = if kind == "run" {
                        None
                    } else {
                        Some(
                            serde_json::from_str::<Value>(&outcome.stdout)
                                .context("Invalid native response")?,
                        )
                    };
                    match kind {
                        "inventory" => {
                            let value = value.context("inventory")?;
                            view.write().devices =
                                serde_json::from_value(value["devices"].clone())?;
                            if let Some(warnings) = value["warnings"].as_array() {
                                view.write().error = warnings
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join("\n");
                            }
                            view.write().status =
                                "Drive discovery is a snapshot, not permission to erase.".into();
                        }
                        "preview" => {
                            let value = value.context("preview")?;
                            let selected = selected.context("selection")?;
                            let summary = value["summary"]
                                .as_str()
                                .context("Missing native summary")?
                                .to_owned();
                            let target = value["target_fingerprint"]
                                .as_str()
                                .context("Missing bound target identity")?
                                .to_owned();
                            preview.set(Some(Preview {
                                config_digest: config_digest(&selected.config)?,
                                selection: selected,
                                summary,
                                target_fingerprint: Some(target),
                            }));
                            erase.set(false);
                            wipe.set(false);
                            native.set(false);
                            page.set(Page::Review);
                        }
                        "catalog" => {
                            view.write().catalog[catalogue_kind.context("catalogue kind")?] =
                                value.context("catalog")?
                        }
                        "account" if account => {
                            if account_configure {
                                view.write().account_result = value;
                            } else {
                                view.write().account_probe = value;
                                fingerprint_confirmed.set(false);
                            }
                        }
                        "run" => {
                            view.write().details = format!("{}{}", outcome.stdout, outcome.stderr);
                            preview.set(None);
                            erase.set(false);
                            wipe.set(false);
                            password.set(String::new());
                            page.set(Page::Result);
                        }
                        _ => return Err(anyhow!("Unknown native controller result")),
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = services.stop_alerts() {
                    view.write().error = error;
                }
                if let Err(error) = result {
                    view.write().error = format!("{error:#}");
                }
                match services.cancellation.lock() {
                    Ok(mut value) => *value = None,
                    Err(error) => {
                        view.write().blocked = true;
                        view.write().error = format!("Cancellation state failed: {error}");
                    }
                }
                view.write().busy = false;
                services.busy.store(false, Ordering::Release);
                if services.closing.load(Ordering::Acquire) {
                    if view.peek().blocked {
                        services.closing.store(false, Ordering::Release);
                        window.set_visible(true);
                    } else {
                        window.set_close_behavior(WindowCloseBehaviour::WindowCloses);
                        window.close();
                    }
                }
            }
        }
    });
    let resolved = normalized.read();
    let validation = resolved
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    let iot = text(&document.read(), "media.imageFamily") == "iot-core";
    let disabled = view.read().busy || view.read().blocked || accessibility() < 0;
    let dry = at_path(&document.read(), "execution.dryRun")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fields = &services.fields;
    let select_services = services.clone();
    let refresh_services = services.clone();
    let catalog_services = services.clone();
    let run_services = services.clone();
    let cancel_services = services.clone();
    let probe_services = services.clone();
    let configure_services = services.clone();
    let mount_services = services.clone();
    let mount_window = window.clone();
    let field_doc = document;
    let field_revision = revision;
    let board_services = services.clone();
    let firmware_services = services.clone();
    let drivers_services = services.clone();
    let permission_services = services.clone();
    let manual_services = services.clone();
    let theme_services = services.clone();
    rsx! {
        style {"{STYLE}"}
        div {class:"app","data-theme":"{appearance}",
        main {onmounted:move |_|{
            if mount_services.smoke {
                println!("WoR-Flasher guided native desktop mounted; no engine commands invoked.");
                if let Some(window)=mount_window.clone(){spawn(async move{
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let result=dioxus::document::eval(r#"
                        const root=document.querySelector(".app");
                        const artwork=root.querySelector(".partnership-art");
                        await artwork.decode();
                        return {
                            theme:root.dataset.theme,
                            canvas:getComputedStyle(root).backgroundColor,
                            dark:matchMedia("(prefers-color-scheme: dark)").matches,
                            width:innerWidth,
                            scrollWidth:document.documentElement.scrollWidth,
                            artwork:{source:artwork.getAttribute("src"),width:artwork.naturalWidth,height:artwork.naturalHeight}
                        };
                    "#).await;
                    let valid=match result {
                        Ok(value)=>{
                            let dark=value["theme"]=="dark"||(value["theme"]=="system"&&value["dark"]==true);
                            let expected=if dark {"rgb(5, 11, 20)"}else{"rgb(242, 246, 252)"};
                            let fits=value["width"].as_u64().zip(value["scrollWidth"].as_u64()).is_some_and(|(width,scroll)|scroll<=width+1);
                            println!("WOR_UI_SMOKE\t{value}");
                            value["canvas"]==expected&&fits&&value["artwork"]["source"]=="assets/partnership.png"&&value["artwork"]["width"]==800&&value["artwork"]["height"]==533
                        },
                        Err(error)=>{eprintln!("Native appearance probe failed: {error}");false}
                    };
                    window.set_close_behavior(WindowCloseBehaviour::WindowCloses);window.close();
                    if !valid {std::process::exit(1);}
                });}
            }
        },
            header {class:"partnership-header",
                div {class:"brand-copy",
                    div {class:"partner-lockup",role:"group",aria_label:"Botspot and Blackout Secure partnership",
                        div {class:"partner-credit",span {class:"brand-creator","Botspot"}small {"Original creator"}}
                        span {class:"partner-plus",aria_hidden:"true","+"}
                        div {class:"partner-credit",span {class:"brand-maintainer","Blackout Secure"}small {"Community maintainer"}}
                    }
                    h1 {"WoR-Flasher"}
                    p {class:"brand-tagline",
                        "Stronger together. "
                        span {class:"brand-future","Better for everyone."}
                    }
                    p {class:"muted brand-caption","Native Rust imaging workflow"}
                }
                img {class:"partnership-art",src:"assets/partnership.png",width:"800",height:"533",
                    alt:"Botspot and Blackout Secure supporting the Windows on Raspberry Pi community"}
                div {class:"header-tools",
                    span {class:"badge","Dioxus / Rust"}
                    div {class:"appearance",
                    label {r#for:"appearance","Appearance"}
                    select {id:"appearance",value:"{appearance}",onchange:move|event|{
                        let result=(||->anyhow::Result<Appearance>{
                            let mode=event.value().parse::<Appearance>()?;
                            let path=theme_services.appearance_path.as_ref().context("Visual fixture preferences are read-only")?;
                            appearance::save(path,mode)?;
                            Ok(mode)
                        })();
                        match result {Ok(mode)=>appearance.set(mode),Err(error)=>view.write().error=format!("Appearance was not saved: {error}")}
                    },option {value:"system","System"}option {value:"light","Light"}option {value:"dark","Dark"}}
                    }
                }
            }
            nav {aria_label:"Imaging steps",
            ol {class:"steps",
                for (step,label) in [(Page::Image,"1 Image & board"),(Page::Drive,"2 Drive"),(Page::Options,"3 Options"),(Page::Review,"4 Confirm"),(Page::Progress,"5 Flash"),(Page::Result,"6 Result")] {
                    li {class:if page()==step {"active"}else{""},aria_current:if page()==step {"step"}else{"false"},"{label}"}
                }
            }}
                if cfg!(target_os="macos")&&accessibility()<0 {
                    section {class:"card",
                        h2 {"macOS disk-alert handling"}
                        p {"Automatic Ignore requires Accessibility permission for this exact app. Permission is queried live and never granted or cached by the application. You may explicitly continue with manual Ignore handling."}
                        div {class:"actions",
                            button {onclick:move |_|if let Err(error)=wor_infrastructure::desktop_system::open_privacy_settings(false){view.write().error=error.to_string();},"Open Accessibility settings"}
                            button {onclick:move |_|{
                                if wor_infrastructure::disk_alerts::accessibility_trusted(){permission_services.alert_auto.store(true,Ordering::Release);accessibility.set(1);}
                                else {view.write().error="Accessibility permission is still unavailable. Add this exact application, then recheck, or continue manually.".into();}
                            },"Recheck"}
                            button {onclick:move |_|{manual_services.alert_auto.store(false,Ordering::Release);accessibility.set(0);},"Continue manually"}
                    }
                }
            }
            if !validation.is_empty(){section {class:"card error","{validation}"}}
            if page()==Page::Image {
                section {class:"card",
                    h2 {"Select Windows and the physical board"}
                    label {r#for:"image-profile","Image profile"}
                    select {id:"image-profile",disabled,value:if iot{"iot-core"}else{"desktop"},onchange:move|event|{
                        switch_family(&mut document.write(),event.value()=="iot-core");revision+=1;
                    },option {value:"desktop","Windows desktop (ARM64)"}option {value:"iot-core","IoT Core (ARM32, legacy)"}}
                    small {"Desktop profiles support Windows 10/11 ARM64; the legacy IoT profile uses reviewed Windows 10 IoT Core ARM32 media."}
                    label {r#for:"target-board","Raspberry Pi"}
                    select {id:"target-board",disabled,value:if iot{text(&document.read(),"target.board")}else{if text(&document.read(),"target.board")=="pi2-v1.2"{"pi2-v1.2".into()}else{text(&document.read(),"target.rpiModel")}},
                        onchange:move|event|{
                            let value=event.value();
                            if let Err(error)=change_board(&board_services,document,revision,&value,iot){view.write().error=format!("Board change was not applied: {error}");}
                        },
                        if iot {option {value:"pi2-v1.1","Raspberry Pi 2 v1.1"}option {value:"pi2-v1.2","Raspberry Pi 2 v1.2"}option {value:"pi3-b","Raspberry Pi 3 Model B"}}
                        else {option {value:"pi2-v1.2","Raspberry Pi 2 v1.2"}option {value:"3","Raspberry Pi 3"}option {value:"4","Raspberry Pi 4 / Pi 400"}option {value:"5","Raspberry Pi 5 (community support)"}}
                    }
                    for field in fields.iter().filter(|field|matches!(field.path.as_str(),"media.sourceFile"|"media.winLang"|"media.bid"|"media.downloadDir"|"media.downloadIotImage")) {
                        if resolved.as_ref().is_ok_and(|config|applicable(field,config,cfg!(target_os="macos"))){Field {field:field.clone(),document:field_doc,revision:field_revision,disabled,normalized}}
                    }
                    label {r#for:"source-picker","Choose a local image using the native file picker"}
                    input {id:"source-picker",r#type:"file",disabled,accept:if iot{".ffu,.iso,.msi"}else{".iso,.esd"},onchange:move|event|{
                        let files=event.files();
                        if let Some(file)=files.first() {
                            let path=file.path();
                            if path.is_absolute() {
                                change(document,revision,"media.sourceFile",Some(json!(path.to_string_lossy())));
                            } else {view.write().error="The file picker did not supply an absolute local path. Enter the full image path above.".into();}
                        }
                    }}
                    p {class:"warning",if iot {"Only reviewed ARM32 FFU/ISO/MSI media and the three listed boards are accepted. Image preparation starts only after confirmation."}else{"Pi 3/Pi 4 builds above 25163 are rejected. Windows and WSL flashing hosts are unsupported."}}
                    div {class:"actions",
                        button {disabled,onclick:move |_|catalog_services.send(Action::Catalog("builds".into())),"Query supported builds"}
                        button {class:"primary",disabled:disabled||!validation.is_empty(),onclick:move |_|page.set(Page::Drive),"Next: target drive"}
                    }
                    if let Some(catalogue)=view.read().catalog.get("builds") {
                        label {r#for:"published-build","Choose a compatible published build"}
                        select {id:"published-build",disabled,value:text(&document.read(),"media.bid"),onchange:move|event|change(document,revision,"media.bid",Some(json!(event.value()))),
                            option {value:"","Select a published build"}
                            for (value,label) in catalog_choices(catalogue){option {value:"{value}","{label}"}}
                        }
                    }
                }
            }
            if page()==Page::Drive {
                section {class:"card",
                    h2 {"Choose the whole drive to erase"}
                    p {class:"warning","Drive discovery does not authorize a later write. Root, source, cache, runtime and log disks are protected again at the destructive boundary."}
                    for device in &view.read().devices {
                        {let path=device.path.to_string_lossy().into_owned();rsx!{button {class:if text(&document.read(),"target.device")==path {"drive selected"}else{"drive"},
                            disabled,onclick:move |_|change(document,revision,"target.device",Some(json!(path.clone()))),"{device.description()}"}}}
                    }
                    if view.read().devices.is_empty(){p {class:"muted","Connect a supported external drive, then refresh. An unknown root topology fails closed."}}
                    if !iot {
                        for field in fields.iter().filter(|field|field.path=="target.canInstallOnSameDrive"){Field {field:field.clone(),document:field_doc,revision:field_revision,disabled,normalized}}
                    }
                    div {class:"actions",
                        button {disabled,onclick:move |_|page.set(Page::Image),"Back"}
                        button {disabled,onclick:move |_|refresh_services.send(Action::Inventory),"Refresh drives"}
                        button {class:"primary",disabled:disabled||text(&document.read(),"target.device").is_empty(),onclick:move |_|{
                            options_original.set(Some(document()));page.set(Page::Options);
                        },"Next: advanced options"}
                    }
                }
            }
            if page()==Page::Options {
                section {class:"card",
                    h2 {"Context-aware advanced options"}
                    p {class:"muted","All options use the same native domain model on Linux and macOS. Passwords are masked and the run configuration is saved only in private temporary files."}
                    for group in ["customization","userAccount","execution","notifications","system","media"] {
                        details {open:group=="execution",summary {"{title(group)}"}
                            div {class:"fields",
                            for field in fields.iter().filter(|field|field.path.starts_with(&format!("{group}."))&&!matches!(field.path.as_str(),"media.sourceFile"|"media.winLang"|"media.bid"|"media.downloadDir"|"media.downloadIotImage"|"customization.configTxt"|"customization.iotHdmiConfig")&&!field.path.starts_with("customization.bootSettings.")) {
                                if resolved.as_ref().is_ok_and(|config|applicable(field,config,cfg!(target_os="macos"))){Field {field:field.clone(),document:field_doc,revision:field_revision,disabled,normalized}}
                            }}
                        }
                    }
                    button {disabled,onclick:move |_|boot_editor.set(true),"Boot Config Editor"}
                    if !iot {
                        div {class:"actions",
                            button {disabled,onclick:move |_|firmware_services.send(Action::Catalog("uefi".into())),"Query compatible firmware"}
                            button {disabled,onclick:move |_|drivers_services.send(Action::Catalog("drivers".into())),"Query compatible drivers"}
                        }
                        for kind in ["uefi","drivers"] {
                            if let Some(catalogue)=view.read().catalog.get(kind) {details {summary {"Compatible {kind} releases"}pre {"{catalog_display(catalogue)}"}}}
                        }
                    }
                    div {class:"actions",
                        button {disabled,onclick:move |_|{
                            if let Some(original)=options_original.read().clone(){document.set(original);revision+=1;}
                            page.set(Page::Drive);
                        },"Discard option changes and go back"}
                        button {class:"primary",disabled:disabled||!validation.is_empty(),onclick:move |_|{
                            match select_services.snapshot(&document()) {
                                Ok(config)=>select_services.send(Action::Preview {selection:Selection {config,device:text(&document(),"target.device").into(),dry_run:dry}}),
                                Err(error)=>view.write().error=format!("Cannot save the private run snapshot: {error}"),
                            }
                        },"Preview exact settings"}
                    }
                }
            }
            if boot_editor(){BootEditor {document,revision,open:boot_editor,normalized}}
            if page()==Page::Review {
                section {class:"card",
                    h2 {"Review and bind erase consent"}
                    if let Some(approved)=preview.read().as_ref(){pre {"{approved.summary}"}}
                    if !dry {
                        p {class:"warning","Native physical writing has not been hardware-qualified. It is explicitly experimental; the stable classic release remains separate."}
                        label {input {r#type:"checkbox",checked:native(),disabled,onchange:move|event|native.set(event.checked())}"I accept the experimental native writer and its remaining hardware qualification."}
                        label {input {r#type:"checkbox",checked:erase(),disabled,onchange:move|event|erase.set(event.checked())}"Erase the selected whole drive and apply the reviewed operations."}
                        if iot&&at_path(&document.read(),"execution.wipeIotDrive")==Some(&json!(true)){
                            label {input {r#type:"checkbox",checked:wipe(),disabled,onchange:move|event|wipe.set(event.checked())}"Separately confirm full-drive zeroing and readback before the image is applied."}
                        }
                        label {r#for:"administrator-password","Administrator password (private input; never saved in the run configuration)"}
                        input {id:"administrator-password",r#type:"password",value:"{password}",disabled,autocomplete:"off",oninput:move|event|password.set(event.value())}
                        p {class:"muted",if accessibility()==1{"Only one exact system unreadable-disk alert is automatically ignored during the active write window. Other or ambiguous alerts remain manual."}else{"Handle macOS unreadable-disk alerts with Ignore manually. Never choose Initialize or Eject during writing."}}
                    } else {p {class:"muted","Dry run prepares and verifies sources only. No drive will be written."}}
                    div {class:"actions",
                        button {disabled,onclick:move |_|page.set(Page::Options),"Back"}
                        button {class:"primary",disabled:disabled||preview.read().is_none()||(!dry&&(!native()||!erase()))||(iot&&at_path(&document.read(),"execution.wipeIotDrive")==Some(&json!(true))&&!dry&&!wipe()),
                            onclick:move |_|{
                                if let Some(approved)=preview.read().clone(){
                                    let credential=password();password.set(String::new());
                                    run_services.send(Action::Run {selection:approved.selection.clone(),preview:approved,confirmed:erase(),wipe:wipe(),native:native(),password:credential});
                                }
                            },if dry {"Start dry run"}else{"Erase and flash"}
                        }
                    }
                }
            }
            if page()==Page::Progress||page()==Page::Result {
                section {class:"card",
                    h2 {if page()==Page::Progress {"Native imaging progress"}else{"Result and qualification"}}
                    p {role:"status",aria_live:"polite","{view.read().status}"}p {"{view.read().step}"}
                    progress {aria_label:"Imaging progress",max:"100",value:"{view.read().percent}"}
                    p {class:"muted","{view.read().task} {view.read().percent:.0}%"}
                    p {class:"muted","Hardware boot: Not Assessed. A dry run or skipped verification is never a verified write."}
                    div {class:"actions",
                        button {class:"danger",disabled:!view.read().busy,onclick:move |_|cancel_services.cancel(),"Cancel operation"}
                        button {disabled,onclick:move |_|page.set(Page::Image),"New image"}
                    }
                    if !view.read().details.is_empty(){details {open:true,summary {"Engine diagnostics and verification"}pre {"{view.read().details}"}}}
                }
                if iot&&page()==Page::Result {
                    section {class:"card",
                        h2 {"Post-boot IoT personalization"}
                        p {class:"muted","Boot the Pi on an isolated network. Connect explicitly, verify its SSH fingerprint, then configure the existing administrator and installed language. No subnet scan is performed."}
                        for field in fields.iter().filter(|field|field.path.starts_with("userAccount.iotCore.")) {
                            if resolved.as_ref().is_ok_and(|config|applicable(field,config,cfg!(target_os="macos"))){Field {field:field.clone(),document:field_doc,revision:field_revision,disabled,normalized}}
                        }
                        button {disabled,onclick:move |_|{
                            match probe_services.resolved(&document()).and_then(|config|account_input(&config,None)) {
                                Ok(input)=>probe_services.send(Action::Account {arguments:vec!["probe".into()],input,configure:false}),
                                Err(error)=>view.write().error=error.to_string(),
                            }
                        },"Connect and inspect SSH identity"}
                        if let Some(probe)=view.read().account_probe.as_ref() {
                            pre {"{public_account_result(probe)}"}
                            label {input {r#type:"checkbox",checked:fingerprint_confirmed(),disabled,onchange:move|event|fingerprint_confirmed.set(event.checked())}"I verified this endpoint and SSH fingerprint."}
                        }
                        button {disabled:disabled||!fingerprint_confirmed(),onclick:move |_|{
                            let probe=view.read().account_probe.clone();
                            match configure_services.resolved(&document()).and_then(|config|account_input(&config,probe.as_ref())) {
                                Ok(input)=>configure_services.send(Action::Account {arguments:vec!["configure".into()],input,configure:true}),
                                Err(error)=>view.write().error=error.to_string(),
                            }
                        },"Apply and verify requested personalization"}
                        if let Some(result)=view.read().account_result.as_ref(){pre {"{public_account_result(result)}"}}
                    }
                }
            }
            if !view.read().error.is_empty(){section {class:"card error",role:"alert","{view.read().error}"}}
            footer {"Original project: Botspot. Maintained with Blackout Secure. Native Rust host; no HTTP server/database. GPL-3.0-or-later; see LICENSE and NOTICE. This application is unsigned/unnotarized."}
        }
        }
    }
}

fn catalog_display(value: &Value) -> String {
    fn public(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(public).collect()),
            Value::Object(values) => Value::Object(
                values
                    .iter()
                    .filter(|(key, _)| {
                        !key.to_ascii_lowercase().contains("url")
                            && !key.to_ascii_lowercase().contains("password")
                    })
                    .map(|(key, value)| (key.clone(), public(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    serde_json::to_string_pretty(&public(value))
        .unwrap_or_else(|_| "Catalogue response could not be rendered".into())
}

fn catalog_choices(value: &Value) -> Vec<(String, String)> {
    fn visit(value: &Value, out: &mut Vec<(String, String)>) {
        match value {
            Value::Array(values) => {
                for value in values {
                    visit(value, out);
                }
            }
            Value::Object(object) => {
                let selected = ["value", "build", "id", "tag", "tag_name"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(Value::as_str));
                if let Some(selected) = selected {
                    let label = ["label", "name", "date"]
                        .iter()
                        .find_map(|key| object.get(*key).and_then(Value::as_str))
                        .unwrap_or(selected);
                    out.push((selected.into(), format!("{selected} - {label}")));
                } else {
                    for (key, value) in object {
                        if !key.to_ascii_lowercase().contains("url") {
                            visit(value, out);
                        }
                    }
                }
            }
            Value::String(value) => {
                if let Some(first) = value.split_whitespace().next()
                    && first
                        .bytes()
                        .all(|character| character.is_ascii_digit() || character == b'.')
                {
                    out.push((first.into(), value.clone()));
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    visit(value, &mut result);
    let mut seen = std::collections::BTreeSet::new();
    result.retain(|(value, _)| seen.insert(value.clone()));
    result
}

fn account_input(config: &NativeConfig, probe: Option<&Value>) -> anyhow::Result<Value> {
    if let Some(probe) = probe {
        if probe.get("fingerprint").and_then(Value::as_str).is_none() {
            return Err(anyhow!("No confirmed SSH fingerprint is available"));
        }
        let mut request = json!({
            "pairing":probe,"confirmIdentity":true,
            "accountSetup":config.flag("IOT_CORE_ACCOUNT_SETUP")?,"accountUsername":config.get("IOT_CORE_ACCOUNT_USERNAME"),
            "accountPassword":config.get("IOT_CORE_ACCOUNT_PASSWORD"),"languageSetup":config.flag("IOT_CORE_LANGUAGE_SETUP")?,
            "language":if config.get("IOT_CORE_LANGUAGE").is_empty(){config.get("WINDOWS_LOCALE")}else{config.get("IOT_CORE_LANGUAGE")}
        });
        for (setting, field) in [
            ("IOT_CORE_CURRENT_USERNAME", "currentUsername"),
            ("IOT_CORE_CURRENT_PASSWORD", "currentPassword"),
        ] {
            if config.is_set(setting) {
                request[field] = json!(config.get(setting));
            }
        }
        Ok(request)
    } else {
        let automatic = if config.get("IOT_CORE_AUTOMATIC_ADDRESS").is_empty() {
            config.get("IOT_CORE_HOST").is_empty()
        } else {
            config.flag("IOT_CORE_AUTOMATIC_ADDRESS")?
        };
        Ok(json!({"host":config.get("IOT_CORE_HOST"),"automaticAddress":automatic}))
    }
}

fn public_account_result(value: &Value) -> String {
    let allowed = [
        "host",
        "address",
        "fingerprint",
        "status",
        "state",
        "username",
        "sid",
        "accountState",
        "languageState",
        "language",
        "verified",
        "pending_reboot",
        "message",
    ];
    let object = allowed
        .into_iter()
        .filter_map(|key| value.get(key).map(|value| (key.to_owned(), value.clone())))
        .collect::<serde_json::Map<_, _>>();
    if object.is_empty() {
        "Personalization: Not Assessed (no recognized verified result)".into()
    } else {
        serde_json::to_string_pretty(&object)
            .unwrap_or_else(|_| "Personalization response could not be rendered".into())
    }
}
