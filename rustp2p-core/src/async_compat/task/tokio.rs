pub use ::tokio;
pub use tokio::spawn;
pub use tokio::time::sleep;
pub use tokio::time::timeout;

#[macro_export]
macro_rules! select {
    ($($t:tt)*) => {
        $crate::async_compat::task::tokio::tokio::select!{$($t)*}
    };
}
