fn main() {
    // `include_dir!` does not register file dependencies on stable, so rebuild
    // whenever any WebUI template or static asset changes.
    println!("cargo:rerun-if-changed=webui");
}
