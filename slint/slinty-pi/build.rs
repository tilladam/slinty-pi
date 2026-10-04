fn main() {
    // `@slint-widgets`: slint-kit's shared components (code block, markdown
    // blocks, command palette, KitStyle).
    let config = slint_build::CompilerConfiguration::new()
        .with_library_paths(slint_widgets::library_paths());
    slint_build::compile_with_config("ui/app.slint", config).expect("slint build failed");

    // No-ops on non-Windows compile targets (checked internally by the crate).
    embed_resource::compile("assets/icon/app.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embedding icon.ico into the exe failed");
}
