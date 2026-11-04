/// Expands to a literal so platform-specific hints can use `concat!` without allocating.
#[macro_export]
macro_rules! alt_key_name {
    () => {
        "alt"
    };
}
