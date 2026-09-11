fn main() {
    cc::Build::new()
        .file("src/syscalls.c")
        .include("include")
        .flag_if_supported("-std=c11")
        .flag_if_supported("-Werror")
        .warnings(true)
        .compile("rustybox_syscalls");

    println!("cargo:rerun-if-changed=include/rustybox.h");
    println!("cargo:rerun-if-changed=src/syscalls.c");
}
