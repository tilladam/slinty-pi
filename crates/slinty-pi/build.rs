fn main() {
    slint_build::compile("ui/app.slint").expect("slint build failed");

    // No-ops on non-Windows compile targets (checked internally by the crate).
    embed_resource::compile("assets/icon/app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embedding icon.ico into the exe failed");
}
