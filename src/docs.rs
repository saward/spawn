//! URLs of published doc pages, for anywhere Spawn produces output — errors,
//! `--help`, warnings.
//!
//! Centralised so a moved page is one edit here, not a hunt through every
//! string naming it. `tests/doc_links.rs` verifies each still resolves.

/// Declares a link and registers it in `ALL`, so a new constant can't escape
/// the link test.
macro_rules! docs {
    ($($(#[$m:meta])* $name:ident = $slug:literal;)*) => {
        $($(#[$m])* pub const $name: &str = concat!("https://docs.spawn.dev/", $slug, "/");)*
        /// Every link above, as (constant name, slug), for `tests/doc_links.rs`.
        pub const ALL: &[(&str, &str)] = &[$((stringify!($name), $slug)),*];
    };
}

docs! {
    /// Defining `[secrets]` and the sources available to them.
    SECRETS = "guides/secrets";
}
