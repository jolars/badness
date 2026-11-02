//! Print generated documentation fragments as JSON.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .ok_or("usage: doc-utils <project-root>")?;
    let fragments = doc_utils::fragments(std::path::Path::new(&root))?;
    serde_json::to_writer(std::io::stdout(), &fragments)?;
    Ok(())
}
