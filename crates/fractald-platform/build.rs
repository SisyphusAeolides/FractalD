fn main() {
    cc::Build::new()
        .file("src/platform.c")
        .include("include")
        .flag_if_supported("-std=c11")
        .warnings(true)
        .compile("fractald_platform");

    println!("cargo:rerun-if-changed=include/fractald_platform.h");
    println!("cargo:rerun-if-changed=src/platform.c");
    println!("cargo:rerun-if-changed=src/syscall_numbers.inc");
}
