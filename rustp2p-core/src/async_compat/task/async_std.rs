pub use async_std::task::spawn;

#[macro_export]
macro_rules! select {
   ($($name:ident = $future:expr => $result:expr $(,)? )*) => {{
        use futures_util::FutureExt;
        futures::select! {
            $(
                $name = $future.fuse() => {
                    $result
                },
            )*
        }
    }};
}

pub use select;
