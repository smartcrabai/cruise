fn main() {
    let target = match std::env::var("TARGET") {
        Ok(target) => target,
        Err(error) => {
            eprintln!("Cargo did not provide the build target: {error}");
            std::process::exit(1);
        }
    };
    println!("cargo:rustc-env=CRUISE_BUILD_TARGET={target}");
    // `include_dir!` does not register file dependencies on stable, so rebuild
    // whenever any WebUI template or static asset changes.
    println!("cargo:rerun-if-changed=webui");
}
