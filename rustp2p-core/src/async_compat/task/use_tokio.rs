pub use ::tokio;
pub use tokio::spawn;

#[macro_export]
macro_rules! select {
    ($($t:tt)*) => {
        $crate::async_compat::task::use_tokio::tokio::select!{$($t)*}
    };
}
