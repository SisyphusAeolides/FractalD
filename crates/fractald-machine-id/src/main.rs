use std::env;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const MACHINE_ID_PATH: &str = "/etc/machine-id";

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("systemd-machine-id-setup: {error}");
            ExitCode::from(1)
        }
    }
}

fn run(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut root = PathBuf::from("/");
    let mut print = false;
    let mut commit = false;
    while let Some(argument) = args.next() {
        if argument == "--root" {
            root = PathBuf::from(
                args.next()
                    .ok_or_else(|| "--root requires a path".to_owned())?,
            );
        } else if let Some(value) = argument.strip_prefix("--root=") {
            if value.is_empty() {
                return Err("--root requires a path".to_owned());
            }
            root = PathBuf::from(value);
        } else if argument == "--print" {
            print = true;
        } else if argument == "--commit" {
            commit = true;
        } else if argument == "--help" || argument == "-h" {
            print_help();
            return Ok(());
        } else if argument == "--version" {
            println!(
                "systemd-machine-id-setup (FractalD) {}",
                env!("CARGO_PKG_VERSION")
            );
            return Ok(());
        } else if argument == "--image" || argument.starts_with("--image=") {
            return Err(
                "--image is unavailable in the native filesystem compatibility tool".to_owned(),
            );
        } else {
            return Err(format!("unsupported option {argument}"));
        }
    }

    let path = rooted_path(&root, MACHINE_ID_PATH);
    let id = ensure_machine_id(&path).map_err(|error| format!("{}: {error}", path.display()))?;
    if print {
        println!("{id}");
    }
    if commit {
        // FractalD writes a persistent machine ID directly. There is no
        // transient systemd machine-id store that needs a second commit.
    }
    Ok(())
}

fn rooted_path(root: &Path, absolute_path: &str) -> PathBuf {
    if root == Path::new("/") {
        PathBuf::from(absolute_path)
    } else {
        root.join(absolute_path.trim_start_matches('/'))
    }
}

fn ensure_machine_id(path: &Path) -> io::Result<String> {
    match fs::read_to_string(path) {
        Ok(contents) => {
            let value = contents.trim();
            if value == "uninitialized" || value.is_empty() {
                return create_machine_id(path);
            }
            if valid_machine_id(value) {
                return Ok(value.to_ascii_lowercase());
            }
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "machine-id is not 32 hexadecimal characters",
            ))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => create_machine_id(path),
        Err(error) => Err(error),
    }
}

fn create_machine_id(path: &Path) -> io::Result<String> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let id = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    fs::write(path, format!("{id}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o444))?;
    }
    Ok(id)
}

fn valid_machine_id(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn print_help() {
    println!(
        "systemd-machine-id-setup (FractalD)\n\nUsage: systemd-machine-id-setup [OPTIONS]\n\n  --root=PATH  initialize an alternate filesystem root\n  --print      print the resulting machine ID\n  --commit     accept the transient-ID commit operation\n  --version    show the version\n  --help       show this help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_root() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = env::temp_dir().join(format!("fractald-machine-id-{stamp}"));
        fs::create_dir_all(path.join("etc")).expect("etc");
        path
    }

    #[test]
    fn creates_and_reuses_an_alternate_root_id() {
        let root = test_root();
        let path = root.join("etc/machine-id");
        let first = ensure_machine_id(&path).expect("machine id");
        assert!(valid_machine_id(&first));
        assert_eq!(ensure_machine_id(&path).expect("same machine id"), first);
        fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn validates_existing_ids() {
        let root = test_root();
        let path = root.join("etc/machine-id");
        fs::write(&path, "not-an-id\n").expect("write invalid id");
        assert_eq!(
            ensure_machine_id(&path)
                .expect_err("invalid id should fail")
                .kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(root).expect("cleanup");
    }
}
