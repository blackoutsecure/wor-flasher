use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use wor_infrastructure::device::{
    DeviceError, DeviceInspector, DeviceSystem, Host, NodeIdentity, NodeKind,
};

struct FakeSystem {
    replies: BTreeMap<String, Value>,
    text: BTreeMap<String, Vec<u8>>,
    nodes: BTreeMap<PathBuf, NodeIdentity>,
    aliases: BTreeMap<PathBuf, PathBuf>,
}

fn key(program: &str, args: &[&str]) -> String {
    format!("{program}|{}", args.join("|"))
}

fn identity(id: u64, kind: NodeKind) -> NodeIdentity {
    NodeIdentity {
        device: 1,
        inode: id,
        rdev: id + 100,
        kind,
    }
}

impl FakeSystem {
    fn new() -> Self {
        Self {
            replies: BTreeMap::new(),
            text: BTreeMap::new(),
            nodes: BTreeMap::new(),
            aliases: BTreeMap::new(),
        }
    }
    fn json(&mut self, program: &str, args: &[&str], value: Value) {
        self.replies.insert(key(program, args), value);
    }
    fn mount(&mut self, path: &str, source: &str, topology: Value) {
        self.text.insert(
            key("findmnt", &["-n", "-o", "SOURCE", "--target", path]),
            source.as_bytes().to_vec(),
        );
        self.json(
            "lsblk",
            &[
                "--inverse",
                "--json",
                "--paths",
                "--output",
                "NAME,TYPE",
                source,
            ],
            topology,
        );
    }
    fn linux_target(&mut self, path: &str, kind: &str, ro: bool) {
        self.nodes
            .insert(path.into(), identity(10, NodeKind::Block));
        self.json("lsblk",&["--json","--bytes","--paths","--nodeps","--output","NAME,TYPE,RO,SIZE,LOG-SEC,PTTYPE,MODEL",path],
            json!({"blockdevices":[{"name":path,"type":kind,"ro":ro,"size":68719476736u64,"log-sec":512,"pttype":"gpt","model":"Fixture disk"}]}));
    }
    fn mac_info(&mut self, path: &str, value: Value) {
        self.json("/usr/sbin/diskutil", &["info", "-plist", path], value);
    }
}

impl DeviceSystem for FakeSystem {
    fn output(
        &self,
        program: &str,
        args: &[OsString],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, DeviceError> {
        if program == "/usr/bin/plutil" {
            return Ok(input.expect("plist input").to_vec());
        }
        let name = key(
            program,
            &args
                .iter()
                .map(|v| v.to_str().expect("arguments"))
                .collect::<Vec<_>>(),
        );
        if let Some(value) = self.replies.get(&name) {
            return Ok(serde_json::to_vec(value).expect("JSON"));
        }
        self.text
            .get(&name)
            .cloned()
            .ok_or_else(|| DeviceError::Probe(format!("Unexpected read-only probe {name}")))
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, DeviceError> {
        Ok(self
            .aliases
            .get(path)
            .cloned()
            .unwrap_or_else(|| path.into()))
    }
    fn node(&self, path: &Path) -> Result<NodeIdentity, DeviceError> {
        self.nodes
            .get(path)
            .cloned()
            .ok_or_else(|| DeviceError::Io {
                path: path.into(),
                source: std::io::ErrorKind::NotFound.into(),
            })
    }
}

fn root_tree() -> Value {
    json!({"blockdevices":[{"name":"/dev/dm-0","type":"lvm","children":[
        {"name":"/dev/dm-1","type":"crypt","children":[{"name":"/dev/nvme0n1","type":"disk"}]},
        {"name":"/dev/nvme1n1","type":"disk"}
    ]}]})
}

#[test]
fn all_layered_linux_root_disks_and_aliases_are_protected() {
    let mut system = FakeSystem::new();
    system.mount("/", "/dev/dm-0", root_tree());
    system
        .aliases
        .insert("/dev/by-id/root-alias".into(), "/dev/nvme0n1".into());
    let inspector = DeviceInspector {
        host: Host::Linux,
        system,
    };
    let roots = inspector
        .backing_disks(Path::new("/"))
        .expect("layered roots");
    assert_eq!(
        roots,
        BTreeSet::from(["/dev/nvme0n1".into(), "/dev/nvme1n1".into()])
    );
    assert!(matches!(
        inspector.inspect(Path::new("/dev/by-id/root-alias"), &roots, false),
        Err(DeviceError::UnsafeTarget(_))
    ));
}

#[test]
fn unknown_or_empty_root_topology_never_becomes_an_empty_allowlist() {
    for topology in [
        json!({"blockdevices":[]}),
        json!({"blockdevices":[{"name":"/dev/dm-0","type":"lvm"}]}),
        json!({"unexpected":[]}),
    ] {
        let mut system = FakeSystem::new();
        system.mount("/", "/dev/dm-0", topology);
        assert!(
            DeviceInspector {
                host: Host::Linux,
                system
            }
            .backing_disks(Path::new("/"))
            .is_err()
        );
    }
    let mut system = FakeSystem::new();
    system.text.insert(
        key("findmnt", &["-n", "-o", "SOURCE", "--target", "/"]),
        b"overlay\n".to_vec(),
    );
    assert!(
        DeviceInspector {
            host: Host::Linux,
            system
        }
        .backing_disks(Path::new("/"))
        .is_err()
    );
}

#[test]
fn partitions_readonly_nodes_and_regular_files_cannot_be_targets() {
    for (kind, ro) in [
        ("part", false),
        ("disk", true),
        ("crypt", false),
        ("lvm", false),
    ] {
        let mut system = FakeSystem::new();
        system.linux_target("/dev/fixture", kind, ro);
        assert!(
            DeviceInspector {
                host: Host::Linux,
                system
            }
            .inspect(Path::new("/dev/fixture"), &BTreeSet::new(), false)
            .is_err()
        );
    }
    let mut system = FakeSystem::new();
    system
        .nodes
        .insert("/dev/not-a-device".into(), identity(1, NodeKind::File));
    assert!(
        DeviceInspector {
            host: Host::Linux,
            system
        }
        .inspect(Path::new("/dev/not-a-device"), &BTreeSet::new(), false)
        .is_err()
    );
}

#[test]
fn iot_rejects_even_an_explicit_loop_target() {
    let mut system = FakeSystem::new();
    system.linux_target("/dev/loop7", "loop", false);
    let inspector = DeviceInspector {
        host: Host::Linux,
        system,
    };
    assert!(
        inspector
            .inspect(Path::new("/dev/loop7"), &BTreeSet::new(), false)
            .is_ok()
    );
    assert!(
        inspector
            .inspect(Path::new("/dev/loop7"), &BTreeSet::new(), true)
            .is_err()
    );
}

#[test]
fn apfs_external_boot_stores_are_resolved_to_every_physical_whole_disk() {
    let mut system = FakeSystem::new();
    system.mac_info("/",json!({"APFSPhysicalStores":[{"APFSPhysicalStore":"disk4s1"},{"APFSPhysicalStore":"disk5s2"}]}));
    system.mac_info("/dev/disk4s1", json!({"ParentWholeDisk":"disk4"}));
    system.mac_info("/dev/disk5s2", json!({"ParentWholeDisk":"disk5"}));
    let inspector = DeviceInspector {
        host: Host::Macos,
        system,
    };
    let roots = inspector
        .backing_disks(Path::new("/"))
        .expect("APFS stores");
    assert_eq!(
        roots,
        BTreeSet::from(["/dev/disk4".into(), "/dev/disk5".into()])
    );
    assert!(
        inspector
            .inspect(Path::new("/dev/disk4"), &roots, false)
            .is_err()
    );
}

#[test]
fn an_apfs_virtual_parent_is_never_mistaken_for_a_protected_physical_disk() {
    let mut system = FakeSystem::new();
    system.mac_info("/", json!({"ParentWholeDisk":"disk3"}));
    system.mac_info("/dev/disk3", json!({"VirtualOrPhysical":"Virtual"}));
    system.json(
        "/usr/sbin/diskutil",
        &["apfs", "list", "-plist", "disk3"],
        json!({"Containers":[{"PhysicalStores":[{"DeviceIdentifier":"disk4s1"}]}]}),
    );
    system.mac_info("/dev/disk4s1", json!({"ParentWholeDisk":"disk4"}));
    assert_eq!(
        DeviceInspector {
            host: Host::Macos,
            system
        }
        .backing_disks(Path::new("/"))
        .expect("store"),
        BTreeSet::from(["/dev/disk4".into()])
    );
}

fn mac_fixture() -> FakeSystem {
    let mut system = FakeSystem::new();
    system
        .nodes
        .insert("/dev/disk6".into(), identity(11, NodeKind::Block));
    system.nodes.insert(
        "/dev/rdisk6".into(),
        identity(u64::MAX - 1000, NodeKind::Character),
    );
    system.mac_info(
        "/dev/disk6",
        json!({"WholeDisk":true,"Internal":false,"VirtualOrPhysical":"Physical",
        "ReadOnlyMedia":false,"WritableMedia":true,"DiskSize":68719476736u64,"DeviceBlockSize":512,
        "Content":"GUID_partition_scheme","MediaName":"Fixture"}),
    );
    system
}

#[test]
fn macos_targets_require_external_physical_writable_whole_disks_and_raw_identity() {
    let system = mac_fixture();
    let snapshot = DeviceInspector {
        host: Host::Macos,
        system,
    }
    .inspect(Path::new("/dev/disk6"), &BTreeSet::new(), true)
    .expect("safe disk");
    assert_eq!(snapshot.raw_path, PathBuf::from("/dev/rdisk6"));
    assert_eq!(snapshot.raw_node.inode, u64::MAX - 1000);
    for (field, value) in [
        ("Internal", json!(true)),
        ("WholeDisk", json!(false)),
        ("VirtualOrPhysical", json!("Virtual")),
        ("ReadOnlyMedia", json!(true)),
    ] {
        let mut system = mac_fixture();
        system
            .replies
            .get_mut(&key(
                "/usr/sbin/diskutil",
                &["info", "-plist", "/dev/disk6"],
            ))
            .expect("metadata")[field] = value;
        assert!(
            DeviceInspector {
                host: Host::Macos,
                system
            }
            .inspect(Path::new("/dev/disk6"), &BTreeSet::new(), true)
            .is_err()
        );
    }
}

#[test]
fn source_runtime_cache_and_log_backing_disks_join_the_protected_set() {
    let mut system = FakeSystem::new();
    system.mount("/", "/dev/dm-0", root_tree());
    system
        .nodes
        .insert("/source.iso".into(), identity(50, NodeKind::File));
    system.mount("/source.iso","/dev/sdb1",json!({"blockdevices":[{"name":"/dev/sdb1","type":"part","children":[{"name":"/dev/sdb","type":"disk"}]}]}));
    system.linux_target("/dev/sdb", "disk", false);
    let inspector = DeviceInspector {
        host: Host::Linux,
        system,
    };
    let protected = inspector
        .protected_disks(&["/source.iso".into()])
        .expect("protected source");
    assert!(protected.contains(Path::new("/dev/sdb")));
    assert!(
        inspector
            .inspect(Path::new("/dev/sdb"), &protected, false)
            .is_err()
    );
}

#[test]
fn device_identity_capacity_sector_and_preview_layout_are_bound_to_consent() {
    let mut system = FakeSystem::new();
    system.mount("/", "/dev/dm-0", root_tree());
    system.linux_target("/dev/sdc", "disk", false);
    let mut inspector = DeviceInspector {
        host: Host::Linux,
        system,
    };
    let before = inspector
        .inspect(Path::new("/dev/sdc"), &BTreeSet::new(), false)
        .expect("initial");
    inspector
        .revalidate(&before, &[], false, true)
        .expect("same target");
    inspector
        .system
        .nodes
        .get_mut(Path::new("/dev/sdc"))
        .expect("node")
        .inode += 1;
    assert!(matches!(
        inspector.revalidate(&before, &[], false, true),
        Err(DeviceError::ChangedTarget)
    ));
    inspector
        .system
        .nodes
        .insert("/dev/sdc".into(), before.node.clone());
    let info = inspector
        .system
        .replies
        .get_mut(&key(
            "lsblk",
            &[
                "--json",
                "--bytes",
                "--paths",
                "--nodeps",
                "--output",
                "NAME,TYPE,RO,SIZE,LOG-SEC,PTTYPE,MODEL",
                "/dev/sdc",
            ],
        ))
        .expect("target");
    info["blockdevices"][0]["pttype"] = json!("dos");
    assert!(inspector.revalidate(&before, &[], false, true).is_err());
    inspector
        .revalidate(&before, &[], false, false)
        .expect("layout may change only after known partitioning");
}
