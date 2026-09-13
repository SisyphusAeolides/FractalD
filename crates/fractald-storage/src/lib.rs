use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};

pub const STORAGE_PREPARE_SERVICE: &str = "storage-prepare";
pub const STORAGE_PROFILE: &str = "storage";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FstabEntry {
    pub source: String,
    pub target: PathBuf,
    pub fstype: String,
    pub options: Vec<String>,
    pub dump: u32,
    pub pass: u32,
    pub line: usize,
}

impl FstabEntry {
    pub fn has_option(&self, name: &str) -> bool {
        self.options.iter().any(|option| option == name)
    }

    pub fn option_value(&self, name: &str) -> Option<&str> {
        self.options.iter().find_map(|option| {
            option
                .strip_prefix(name)
                .and_then(|value| value.strip_prefix('='))
        })
    }

    pub fn noauto(&self) -> bool {
        self.has_option("noauto")
    }

    pub fn nofail(&self) -> bool {
        self.has_option("nofail")
    }

    pub fn network(&self) -> bool {
        let filesystem = self.fstype.to_ascii_lowercase();
        self.has_option("_netdev")
            || matches!(
                filesystem.as_str(),
                "9p" | "ceph"
                    | "cifs"
                    | "davfs"
                    | "glusterfs"
                    | "lustre"
                    | "nfs"
                    | "nfs4"
                    | "smb3"
                    | "smbfs"
                    | "sshfs"
                    | "fuse.ceph"
                    | "fuse.glusterfs"
                    | "fuse.sshfs"
            )
    }

    pub fn swap(&self) -> bool {
        self.fstype.eq_ignore_ascii_case("swap")
    }

    pub fn pseudo(&self) -> bool {
        matches!(
            self.fstype.to_ascii_lowercase().as_str(),
            "autofs"
                | "bpf"
                | "binder"
                | "binfmt_misc"
                | "cgroup"
                | "cgroup2"
                | "configfs"
                | "debugfs"
                | "devpts"
                | "devtmpfs"
                | "efivarfs"
                | "fusectl"
                | "fuse"
                | "fuseblk"
                | "hugetlbfs"
                | "mqueue"
                | "nfsd"
                | "overlay"
                | "pipefs"
                | "proc"
                | "pstore"
                | "ramfs"
                | "rpc_pipefs"
                | "securityfs"
                | "selinuxfs"
                | "sockfs"
                | "sysfs"
                | "tmpfs"
                | "tracefs"
        )
    }

    pub fn mount_options(&self) -> Vec<String> {
        self.options
            .iter()
            .filter(|option| {
                !matches!(
                    option.as_str(),
                    "defaults" | "auto" | "noauto" | "nofail" | "_netdev"
                ) && !option.starts_with("comment=")
                    && !option.starts_with("x-")
            })
            .cloned()
            .collect()
    }

    pub fn parent_targets(&self) -> impl Iterator<Item = PathBuf> {
        let mut parents = Vec::new();
        let mut current = PathBuf::new();
        for component in self.target.components() {
            match component {
                Component::RootDir => current.push("/"),
                Component::Normal(value) => {
                    current.push(value);
                    if current != self.target {
                        parents.push(current.clone());
                    }
                }
                _ => {}
            }
        }
        parents.into_iter()
    }

    pub fn mount_service_name(&self) -> String {
        mount_service_name(&self.target)
    }

    pub fn swap_service_name(&self) -> String {
        let value = self
            .source
            .split('/')
            .filter(|component| !component.is_empty())
            .map(|component| escape_component(component.as_bytes()))
            .collect::<Vec<_>>()
            .join("-");
        format!("swap-{value}")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CrypttabEntry {
    pub name: String,
    pub source: String,
    pub key: Option<String>,
    pub options: Vec<String>,
    pub line: usize,
}

impl CrypttabEntry {
    pub fn noauto(&self) -> bool {
        self.options.iter().any(|option| option == "noauto")
    }

    pub fn nofail(&self) -> bool {
        self.options.iter().any(|option| option == "nofail")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GeneratedService {
    pub name: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageError {
    pub line: Option<usize>,
    pub message: String,
}

impl StorageError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(line) = self.line {
            write!(formatter, "line {line}: {}", self.message)
        } else {
            formatter.write_str(&self.message)
        }
    }
}

impl std::error::Error for StorageError {}

pub fn parse_fstab(source: &str) -> Result<Vec<FstabEntry>, StorageError> {
    parse_table(source, false).map(|entries| {
        entries
            .into_iter()
            .map(|entry| FstabEntry {
                source: entry.fields[0].clone(),
                target: PathBuf::from(&entry.fields[1]),
                fstype: entry.fields[2].clone(),
                options: split_options(&entry.fields[3]),
                dump: entry
                    .fields
                    .get(4)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                pass: entry
                    .fields
                    .get(5)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0),
                line: entry.line,
            })
            .collect()
    })
}

pub fn parse_crypttab(source: &str) -> Result<Vec<CrypttabEntry>, StorageError> {
    parse_table(source, true).map(|entries| {
        entries
            .into_iter()
            .map(|entry| CrypttabEntry {
                name: entry.fields[0].clone(),
                source: entry.fields[1].clone(),
                key: entry
                    .fields
                    .get(2)
                    .filter(|value| !value.is_empty() && value.as_str() != "none")
                    .cloned(),
                options: entry
                    .fields
                    .get(3)
                    .map_or_else(Vec::new, |value| split_options(value)),
                line: entry.line,
            })
            .collect()
    })
}

pub fn generate_services(entries: &[FstabEntry]) -> Result<Vec<GeneratedService>, StorageError> {
    let mut mounts = entries
        .iter()
        .filter(|entry| !entry.noauto() && !entry.swap())
        .filter(|entry| entry.target != Path::new("/"))
        .collect::<Vec<_>>();
    let mut swaps = entries
        .iter()
        .filter(|entry| !entry.noauto() && entry.swap())
        .collect::<Vec<_>>();

    let mut target_names = BTreeMap::new();
    for entry in &mounts {
        let name = entry.mount_service_name();
        if let Some(previous) = target_names.insert(entry.target.clone(), name.clone()) {
            return Err(StorageError::at(
                entry.line,
                format!(
                    "duplicate active mount target {} (already represented by {previous})",
                    entry.target.display()
                ),
            ));
        }
    }

    mounts.sort_by_key(|entry| {
        (
            entry.target.components().count(),
            entry.target.clone(),
            entry.line,
        )
    });
    swaps.sort_by_key(|entry| (entry.source.clone(), entry.line));

    if mounts.is_empty() && swaps.is_empty() {
        return Ok(Vec::new());
    }

    let active_targets = mounts
        .iter()
        .map(|entry| entry.target.clone())
        .collect::<BTreeSet<_>>();
    let mut units = Vec::new();
    units.push(GeneratedService {
        name: format!("{STORAGE_PREPARE_SERVICE}.svc"),
        source: storage_prepare_service(),
    });

    for entry in &mounts {
        units.push(GeneratedService {
            name: format!("{}.svc", entry.mount_service_name()),
            source: mount_service(entry, &active_targets),
        });
    }
    for entry in &swaps {
        units.push(GeneratedService {
            name: format!("{}.svc", entry.swap_service_name()),
            source: swap_service(entry),
        });
    }

    let mut wanted = vec![STORAGE_PREPARE_SERVICE.to_owned()];
    wanted.extend(mounts.iter().map(|entry| entry.mount_service_name()));
    wanted.extend(swaps.iter().map(|entry| entry.swap_service_name()));
    let mut after = wanted.clone();
    after.sort();
    units.push(GeneratedService {
        name: format!("{STORAGE_PROFILE}.svc"),
        source: format!(
            "[service]\nkind=group\nremain_after_exit=true\n\n[dependencies]\nwants={}\nafter={}\n",
            wanted.join(" "),
            after.join(" ")
        ),
    });

    units.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(units)
}

fn parse_table(source: &str, crypttab: bool) -> Result<Vec<TableEntry>, StorageError> {
    let mut entries = Vec::new();
    for (index, physical) in source.lines().enumerate() {
        let line = index + 1;
        let content = strip_comment(physical).trim();
        if content.is_empty() {
            continue;
        }
        let raw_fields = content.split_whitespace().collect::<Vec<_>>();
        let minimum = if crypttab { 2 } else { 4 };
        if raw_fields.len() < minimum || raw_fields.len() > 6 {
            return Err(StorageError::at(
                line,
                format!("expected {minimum} to 6 fields, found {}", raw_fields.len()),
            ));
        }
        let mut fields = Vec::with_capacity(raw_fields.len());
        for raw in raw_fields {
            let decoded = decode_field(raw, line)?;
            if decoded.chars().any(char::is_control) {
                return Err(StorageError::at(line, "field contains a control character"));
            }
            fields.push(decoded);
        }
        if !crypttab {
            if fields[1] == "none" && !fields[2].eq_ignore_ascii_case("swap") {
                return Err(StorageError::at(
                    line,
                    "mount target none is valid only for swap entries",
                ));
            }
            if fields[1] != "none" && !fields[1].starts_with('/') {
                return Err(StorageError::at(
                    line,
                    format!("mount target is not absolute: {}", fields[1]),
                ));
            }
            if fields.len() >= 5 && fields[4].parse::<u32>().is_err() {
                return Err(StorageError::at(
                    line,
                    "dump field is not an unsigned integer",
                ));
            }
            if fields.len() >= 6 && fields[5].parse::<u32>().is_err() {
                return Err(StorageError::at(
                    line,
                    "pass field is not an unsigned integer",
                ));
            }
        }
        entries.push(TableEntry { fields, line });
    }
    Ok(entries)
}

#[derive(Clone, Debug)]
struct TableEntry {
    fields: Vec<String>,
    line: usize,
}

fn strip_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        let mut slashes = 0;
        let mut cursor = index;
        while cursor > 0 && bytes[cursor - 1] == b'\\' {
            slashes += 1;
            cursor -= 1;
        }
        if slashes % 2 == 0 {
            return &value[..index];
        }
    }
    value
}

fn decode_field(value: &str, line: usize) -> Result<String, StorageError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &bytes[index + 1..index + 4];
            if digits.iter().all(|digit| (b'0'..=b'7').contains(digit)) {
                decoded.push((digits[0] - b'0') * 64 + (digits[1] - b'0') * 8 + (digits[2] - b'0'));
                index += 4;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8(decoded)
        .map_err(|_| StorageError::at(line, "field is not valid UTF-8 after decoding"))
}

fn split_options(value: &str) -> Vec<String> {
    value
        .split(',')
        .filter(|option| !option.is_empty())
        .map(str::to_owned)
        .collect()
}

fn mount_service_name(path: &Path) -> String {
    let value = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(escape_component(value.to_string_lossy().as_bytes())),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("-");
    format!("mount-{value}")
}

fn escape_component(value: &[u8]) -> String {
    value
        .iter()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':') {
                (*byte as char).to_string()
            } else {
                format!("\\x{byte:02x}")
            }
        })
        .collect()
}

fn storage_prepare_service() -> String {
    "[service]\nkind=oneshot\nexec=/usr/bin/fractald storage-prepare\nremain_after_exit=true\nstart_timeout=120s\n".to_owned()
}

fn mount_service(entry: &FstabEntry, active_targets: &BTreeSet<PathBuf>) -> String {
    let mut requires = Vec::new();
    let mut wants = vec![STORAGE_PREPARE_SERVICE.to_owned()];
    let mut after = vec![STORAGE_PREPARE_SERVICE.to_owned()];
    let mut before = Vec::new();
    if entry.network() {
        wants.push("network-online".to_owned());
        after.push("network-online".to_owned());
    }
    if !entry.nofail() {
        if let Some(device) = device_service_name(&entry.source) {
            requires.push(device.clone());
            after.push(device);
        }
    }
    for parent in entry.parent_targets() {
        if !active_targets.contains(&parent) {
            continue;
        }
        let name = mount_service_name(&parent);
        if entry.nofail() {
            wants.push(name.clone());
        } else {
            requires.push(name.clone());
        }
        after.push(name);
    }
    for option in &entry.options {
        if let Some(value) = option.strip_prefix("x-fractald.requires=") {
            requires.extend(value.split_whitespace().map(str::to_owned));
        } else if let Some(value) = option.strip_prefix("x-fractald.wants=") {
            wants.extend(value.split_whitespace().map(str::to_owned));
        } else if let Some(value) = option.strip_prefix("x-fractald.after=") {
            after.extend(value.split_whitespace().map(str::to_owned));
        } else if let Some(value) = option.strip_prefix("x-fractald.before=") {
            before.extend(value.split_whitespace().map(str::to_owned));
        }
    }
    let mut source = String::new();
    source.push_str("[service]\nkind=mount\nremain_after_exit=true\n");
    source.push_str("\n[dependencies]\n");
    push_words(&mut source, "requires", &requires);
    push_words(&mut source, "wants", &wants);
    push_words(&mut source, "after", &after);
    push_words(&mut source, "before", &before);
    source.push_str("\n[conditions]\nmount_point=!");
    source.push_str(&entry.target.to_string_lossy());
    source.push_str("\n\n[mount]\nwhat=");
    source.push_str(&entry.source);
    source.push_str("\nwhere=");
    source.push_str(&entry.target.to_string_lossy());
    let fstype = canonical_filesystem_type(&entry.fstype);
    if !fstype.eq_ignore_ascii_case("auto") {
        source.push_str("\ntype=");
        source.push_str(&fstype);
    }
    let options = entry.mount_options();
    if !options.is_empty() {
        source.push_str("\noptions=");
        source.push_str(&options.join(","));
    }
    if let Some(value) = entry
        .option_value("x-fractald.mount-timeout")
        .or_else(|| entry.option_value("x-fractald.device-timeout"))
    {
        source.push_str("\ntimeout=");
        source.push_str(value);
    }
    source.push_str("\ndirectory_mode=0755\n");
    source
}

fn canonical_filesystem_type(value: &str) -> String {
    match value.to_ascii_lowercase().as_str() {
        "fat" | "msdos" => "vfat".to_owned(),
        "ext" => "ext4".to_owned(),
        value => value.to_owned(),
    }
}

fn device_service_name(source: &str) -> Option<String> {
    if !source.starts_with("/dev/") {
        return None;
    }
    let stem = source
        .split('/')
        .filter(|component| !component.is_empty())
        .map(|component| escape_component(component.as_bytes()))
        .collect::<Vec<_>>()
        .join("-");
    (!stem.is_empty()).then(|| format!("device-{stem}"))
}

fn swap_service(entry: &FstabEntry) -> String {
    let mut requires = Vec::new();
    let mut wants = vec![STORAGE_PREPARE_SERVICE.to_owned()];
    let mut after = vec![STORAGE_PREPARE_SERVICE.to_owned()];
    if !entry.nofail() {
        if let Some(device) = device_service_name(&entry.source) {
            requires.push(device.clone());
            after.push(device);
        }
    }
    for option in &entry.options {
        if let Some(value) = option.strip_prefix("x-fractald.requires=") {
            requires.extend(value.split_whitespace().map(str::to_owned));
        } else if let Some(value) = option.strip_prefix("x-fractald.wants=") {
            wants.extend(value.split_whitespace().map(str::to_owned));
        } else if let Some(value) = option.strip_prefix("x-fractald.after=") {
            after.extend(value.split_whitespace().map(str::to_owned));
        }
    }
    let mut source = String::new();
    source.push_str("[service]\nkind=swap\nremain_after_exit=true\n\n[dependencies]\n");
    push_words(&mut source, "requires", &requires);
    push_words(&mut source, "wants", &wants);
    push_words(&mut source, "after", &after);
    source.push_str("\n[swap]\nwhat=");
    source.push_str(&entry.source);
    let options = entry.mount_options();
    if !options.is_empty() {
        source.push_str("\noptions=");
        source.push_str(&options.join(","));
    }
    if let Some(priority) = entry.option_value("pri") {
        source.push_str("\npriority=");
        source.push_str(priority);
    }
    source.push('\n');
    source
}

fn push_words(source: &mut String, key: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    source.push_str(key);
    source.push('=');
    source.push_str(&values.join(" "));
    source.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_filesystems_and_fstab_escapes() {
        let entries = parse_fstab(
            r#"
# source target type options dump pass
UUID=pool /srv/pool btrfs defaults,compress=zstd:1 0 0
/dev/mapper/data /srv/data\040pool xfs noatime,nofail 0 2
LABEL=EFI /boot/efi vfat umask=0077 0 0
server:/export /mnt/nfs nfs4 _netdev,x-fractald.after=network-online 0 0
//server/share /mnt/cifs cifs credentials=/etc/cifs.credentials 0 0
/swapfile none swap defaults,pri=20 0 0
"#,
        )
        .expect("fstab");
        assert_eq!(entries.len(), 6);
        assert_eq!(entries[1].target, PathBuf::from("/srv/data pool"));
        assert!(entries[1].nofail());
        assert!(entries[3].network());
        assert!(entries[4].network());
        assert!(entries[5].swap());
        assert_eq!(entries[5].option_value("pri"), Some("20"));
    }

    #[test]
    fn decodes_backslash_escaped_fstab_fields() {
        let entries = parse_fstab(r#"LABEL=pool /srv/data\040pool\134archive xfs defaults 0 0"#)
            .expect("fstab");
        assert_eq!(entries[0].target, PathBuf::from("/srv/data pool\\archive"));
    }

    #[test]
    fn generates_native_storage_services_and_parent_order() {
        let entries = parse_fstab(
            "/dev/md0 /srv btrfs defaults 0 0\nUUID=pool /srv/data btrfs defaults 0 0\n/dev/mapper/vg-lv /srv/data/cache ext4 noauto 0 0\n",
        )
        .expect("fstab");
        let services = generate_services(&entries).expect("services");
        let names = services
            .iter()
            .map(|service| service.name.as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"storage-prepare.svc"));
        assert!(names.contains(&"storage.svc"));
        let parent = services
            .iter()
            .find(|service| service.name == "mount-srv.svc")
            .expect("parent");
        let child = services
            .iter()
            .find(|service| service.name == "mount-srv-data.svc")
            .expect("child");
        assert!(child.source.contains("requires=mount-srv"));
        assert!(child.source.lines().any(|line| line.starts_with("after=")
            && line.split_whitespace().any(|value| value == "mount-srv")));
        assert!(!names.contains(&"mount-srv-data-cache.svc"));
        assert!(parent.source.contains("type=btrfs"));
        assert!(parent.source.starts_with("[service]"));
        assert!(!parent.source.contains("[Unit]"));
    }

    #[test]
    fn adds_native_device_ordering_for_direct_block_sources() {
        let entries = parse_fstab(
            "/dev/mapper/vg-data /srv/data xfs defaults 0 0\nUUID=pool /srv/pool btrfs defaults 0 0\n/dev/vdb1 /srv/optional ext4 nofail 0 0\n",
        )
        .expect("fstab");
        let services = generate_services(&entries).expect("services");
        let mapper = services
            .iter()
            .find(|service| service.name == "mount-srv-data.svc")
            .expect("mapper mount");
        assert!(
            mapper
                .source
                .contains(r"requires=device-dev-mapper-vg\x2ddata")
        );
        assert!(mapper.source.lines().any(
            |line| line.starts_with("after=") && line.contains(r"device-dev-mapper-vg\x2ddata")
        ));

        let by_uuid = services
            .iter()
            .find(|service| service.name == "mount-srv-pool.svc")
            .expect("UUID mount");
        assert!(by_uuid.source.contains("what=UUID=pool"));
        assert!(!by_uuid.source.contains("device-"));

        let optional = services
            .iter()
            .find(|service| service.name == "mount-srv-optional.svc")
            .expect("optional mount");
        assert!(!optional.source.contains("device-"));

        let swap = generate_services(
            &parse_fstab("/dev/zram0 none swap defaults 0 0").expect("swap fstab"),
        )
        .expect("swap services")
        .into_iter()
        .find(|service| service.name.starts_with("swap-") && service.name.ends_with(".svc"))
        .expect("swap service");
        assert!(swap.source.contains("requires=device-dev-zram0"));
    }

    #[test]
    fn honors_noauto_nofail_and_crypttab_fields() {
        let entries =
            parse_fstab("/dev/vda1 /mnt/ext ext4 noauto 0 0\n/dev/vda2 /mnt/xfs xfs nofail 0 0\n")
                .expect("fstab");
        let services = generate_services(&entries).expect("services");
        assert!(
            services
                .iter()
                .all(|service| !service.name.contains("mnt-ext"))
        );
        let xfs = services
            .iter()
            .find(|service| service.name == "mount-mnt-xfs.svc")
            .expect("xfs");
        assert!(!xfs.source.contains("nofail"));
        let crypt = parse_crypttab("cryptdata UUID=abcd /etc/keys/data.key luks,discard\n")
            .expect("crypttab");
        assert_eq!(crypt[0].name, "cryptdata");
        assert_eq!(crypt[0].key.as_deref(), Some("/etc/keys/data.key"));
        assert!(crypt[0].options.contains(&"luks".to_owned()));
    }

    #[test]
    fn canonicalizes_common_filesystem_aliases() {
        let entries = parse_fstab(
            "/dev/vda1 /mnt/fat fat defaults 0 0\n/dev/vda2 /mnt/msdos msdos defaults 0 0\n/dev/vda3 /mnt/ext ext defaults 0 0\n",
        )
        .expect("fstab");
        let services = generate_services(&entries).expect("services");
        let fat = services
            .iter()
            .find(|service| service.name == "mount-mnt-fat.svc")
            .expect("fat mount");
        let ext = services
            .iter()
            .find(|service| service.name == "mount-mnt-ext.svc")
            .expect("ext mount");
        let msdos = services
            .iter()
            .find(|service| service.name == "mount-mnt-msdos.svc")
            .expect("msdos mount");
        assert!(fat.source.contains("type=vfat"));
        assert!(msdos.source.contains("type=vfat"));
        assert!(ext.source.contains("type=ext4"));
    }

    #[test]
    fn classifies_pseudo_filesystems_without_rejecting_them() {
        for filesystem in [
            "bpf", "binder", "configfs", "debugfs", "fuse", "fusectl", "overlay",
        ] {
            let entry = FstabEntry {
                source: filesystem.to_owned(),
                target: PathBuf::from(format!("/mnt/{filesystem}")),
                fstype: filesystem.to_owned(),
                options: Vec::new(),
                dump: 0,
                pass: 0,
                line: 1,
            };
            assert!(entry.pseudo(), "{filesystem} should be pseudo");
            let services = generate_services(&[entry]).expect("pseudo filesystem service");
            assert!(
                services
                    .iter()
                    .any(|service| service.name.ends_with(".svc"))
            );
        }
    }
}
