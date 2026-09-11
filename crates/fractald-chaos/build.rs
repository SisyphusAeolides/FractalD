fn main() {
    cc::Build::new()
        .file("src/kernels.c")
        .include("include")
        .flag_if_supported("-std=c11")
        .warnings(true)
        .compile("fractald_chaos_kernels");

    println!("cargo:rerun-if-changed=include/fractald_chaos.h");
    println!("cargo:rerun-if-changed=src/kernels.c");
}
