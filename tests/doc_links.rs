//! Starlight derives a page's URL from its path under
//! `docs/src/content/docs`, so checking the file checks the URL.

use std::path::Path;

#[test]
fn every_doc_link_points_at_a_real_page() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/src/content/docs");

    for (name, slug) in spawn_db::docs::ALL {
        let page = root.join(slug);
        assert!(
            ["md", "mdx"].iter().any(|ext| page.with_extension(ext).exists()),
            "docs::{name} points at /{slug}/, but no docs/src/content/docs/{slug}.md or .mdx exists"
        );
    }
}
