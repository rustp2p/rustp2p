use std::future::Future;
#[cfg(feature = "use-async-std")]
use futures_util::FutureExt;

pub struct Elapsed;
#[cfg(feature = "use-tokio")]
pub async fn timeout<F:Future>(duration:std::time::Duration,f:F)->Result<F::Output,Elapsed>{
    tokio::time::timeout(duration,f).await.map_err(|_|Elapsed)
}
#[cfg(feature = "use-async-std")]
pub async fn timeout<F:Future>(duration:std::time::Duration,f:F)->Result<F::Output,Elapsed>{
    futures::select! {
        _ = async {
            async_std::task::sleep(duration).await
        }.fuse() =>{
            Err(Elapsed)
        }
        result = f.fuse() =>{
            Ok(result)
        }
    }
}

#[cfg(feature = "use-tokio")]
pub async fn sleep(duration:std::time::Duration){
    tokio::time::sleep(duration).await
}

#[cfg(feature = "use-async-std")]
pub async fn sleep(duration:std::time::Duration){
    async_std::task::sleep(duration).await
}