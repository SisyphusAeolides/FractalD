use std::env;
use std::io::{self, Read};
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("fractald-escape: {error}");
            ExitCode::from(1)
        }
    }
}

#[derive(Debug, Default)]
struct Options {
    unescape: bool,
    mangle: bool,
    path: bool,
    instance: bool,
    suffix: Option<String>,
    template: Option<String>,
    exit: bool,
}

fn run() -> Result<(), String> {
    let (options, values) = parse_args()?;
    if options.exit {
        return Ok(());
    }
    let values = if values.is_empty() {
        let mut input = String::new();
        io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| format!("cannot read stdin: {error}"))?;
        input.lines().map(str::to_owned).collect::<Vec<_>>()
    } else {
        values
    };
    if values.is_empty() {
        return Err("no names were provided".to_owned());
    }
    for value in values {
        let escaped = transform(&value, &options)?;
        println!("{escaped}");
    }
    Ok(())
}

fn parse_args() -> Result<(Options, Vec<String>), String> {
    let mut options = Options::default();
    let mut values = Vec::new();
    let mut arguments = env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                print_help();
                options.exit = true;
                return Ok((options, Vec::new()));
            }
            "--version" => {
                println!("fractald-escape (FractalD) 0.1.0");
                options.exit = true;
                return Ok((options, Vec::new()));
            }
            "--unescape" | "-u" => options.unescape = true,
            "--mangle" | "-m" => options.mangle = true,
            "--path" | "-p" => options.path = true,
            "--instance" => options.instance = true,
            "--suffix" => {
                options.suffix = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--suffix requires a value".to_owned())?,
                );
            }
            value if value.starts_with("--suffix=") => {
                options.suffix = Some(value.trim_start_matches("--suffix=").to_owned());
            }
            "--template" => {
                options.template = Some(
                    arguments
                        .next()
                        .ok_or_else(|| "--template requires a value".to_owned())?,
                );
            }
            value if value.starts_with("--template=") => {
                options.template = Some(value.trim_start_matches("--template=").to_owned());
            }
            value if value.starts_with('-') => {
                return Err(format!("unsupported option {value}"));
            }
            value => values.push(value.to_owned()),
        }
    }
    if options.unescape && (options.suffix.is_some() || options.template.is_some()) {
        return Err("--suffix and --template require escaping mode".to_owned());
    }
    if options.suffix.is_some() && options.template.is_some() {
        return Err("--suffix= and --template= may not be combined".to_owned());
    }
    if options.instance && !options.unescape {
        return Err("--instance requires --unescape".to_owned());
    }
    if options.mangle && options.path {
        return Err("--mangle may not be combined with --path".to_owned());
    }
    Ok((options, values))
}

fn transform(value: &str, options: &Options) -> Result<String, String> {
    if options.unescape {
        if options.path {
            return path_unescape(value);
        }
        let decoded = unescape(value)?;
        if options.instance {
            return Ok(instance_part(&decoded).to_owned());
        }
        return Ok(decoded);
    }
    let mut output = if options.path {
        path_escape(value)
    } else if options.mangle {
        mangle(value)
    } else {
        escape(value)
    };
    if let Some(template) = &options.template {
        output = template_instance(template, &output);
    }
    if let Some(suffix) = &options.suffix {
        let suffix = suffix.trim_start_matches('.');
        if !suffix.is_empty() {
            output.push('.');
            output.push_str(suffix);
        }
    }
    Ok(output)
}

fn escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b':') {
                (byte as char).to_string()
            } else {
                format!("\\x{byte:02x}")
            }
        })
        .collect()
}

fn path_escape(value: &str) -> String {
    let mut output = String::new();
    let mut components = value.split('/').filter(|component| !component.is_empty());
    for (index, component) in components.by_ref().enumerate() {
        if index > 0 {
            output.push('-');
        }
        output.push_str(&escape(component));
    }
    if output.is_empty() {
        "-".to_owned()
    } else {
        output
    }
}

fn mangle(value: &str) -> String {
    value
        .chars()
        .map(|character| if character == '/' { '-' } else { character })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

fn unescape(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' {
            if bytes.get(index + 1) != Some(&b'x') || index + 3 >= bytes.len() {
                return Err(format!("invalid escape at byte {index}"));
            }
            let value = u8::from_str_radix(&value[index + 2..index + 4], 16)
                .map_err(|error| format!("invalid escape at byte {index}: {error}"))?;
            output.push(value);
            index += 4;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(output).map_err(|error| format!("escaped value is not UTF-8: {error}"))
}

fn path_unescape(value: &str) -> Result<String, String> {
    if value == "-" {
        return Ok("/".to_owned());
    }
    Ok(format!(
        "/{}",
        value
            .split('-')
            .map(unescape)
            .collect::<Result<Vec<_>, _>>()?
            .join("/")
    ))
}

fn instance_part(value: &str) -> &str {
    let value = value.rsplit_once('.').map_or(value, |(base, _)| base);
    value
        .rsplit_once('@')
        .map_or(value, |(_, instance)| instance)
}

fn template_instance(template: &str, instance: &str) -> String {
    if template.contains("%i") {
        return template.replace("%i", instance);
    }
    if let Some(index) = template.find('@') {
        let suffix = template[index + 1..].trim_start_matches('@');
        return format!("{}@{}{}", &template[..index], instance, suffix);
    }
    format!("{template}@{instance}")
}

fn print_help() {
    println!(
        "fractald-escape (FractalD)\n\nusage: fractald-escape [OPTIONS] [NAME...]\n\n  -u, --unescape       decode escaped names\n  -m, --mangle         replace path separators without escaping\n  -p, --path           treat names as paths\n      --instance       print only the instance when unescaping\n      --suffix=SUFFIX   append a unit suffix\n      --template=NAME   insert each name into a template"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_common_unit_name_characters() {
        assert_eq!(escape("foo bar"), r"foo\x20bar");
        assert_eq!(path_escape("/var/lib/foo-bar"), r"var-lib-foo\x2dbar");
    }

    #[test]
    fn applies_templates_and_suffixes() {
        let options = Options {
            template: Some("worker@.svc".to_owned()),
            ..Options::default()
        };
        assert_eq!(
            transform("alpha", &options).expect("template"),
            "worker@alpha.svc"
        );
    }

    #[test]
    fn unescapes_paths_and_instances() {
        assert_eq!(path_unescape("var-lib-foo").expect("path"), "/var/lib/foo");
        assert_eq!(
            path_unescape(r"var-lib-foo\x2dbar").expect("escaped path"),
            "/var/lib/foo-bar"
        );
        assert_eq!(instance_part("worker@alpha.svc"), "alpha");
    }
}
