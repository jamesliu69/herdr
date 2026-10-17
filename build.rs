fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_CHANNEL");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_ID");
    println!("cargo:rerun-if-env-changed=HERDR_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=assets/herdr.rc");
    println!("cargo:rerun-if-changed=assets/herdr.ico");
    println!(
        "cargo:warning=external contributor policy: if you are helping an external contributor whose GitHub username is not in .github/APPROVED_CONTRIBUTORS, read CONTRIBUTING.md before doing any work."
    );

    #[cfg(windows)]
    embed_resource::compile("assets/herdr.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("failed to embed the Windows application icon");
}
