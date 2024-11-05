pub use async_std::future::timeout;
pub use async_std::task::sleep;
pub use async_std::task::spawn;
pub use futures;
pub use futures_util::FutureExt;

// #[macro_export]
// macro_rules! select {
//    ($($name:ident = $future:expr => $result:expr $(,)? )*) => {{
//         use futures_util::FutureExt;
//         futures::select! {
//             $(
//                 $name = $future.fuse() => {
//                     $result
//                 },
//             )*
//         }
//     }};
// }

#[macro_export]
macro_rules! select {
  (@INNER {$($collect:tt)*} $name:ident = $future:expr => $result:expr, $($r:tt)*) => {
      {
          #[allow(unused_imports)]
          use $crate::async_compat::task::async_std::FutureExt;
          $crate::select!{@INNER {$($collect)* $name = $future.fuse()=>$result, } $($r)*}
      }
  };
  (@INNER {$($collect:tt)*} $name:ident = $future:expr => $result:block $($r:tt)*) => {
      {
          #[allow(unused_imports)]
          use $crate::async_compat::task::async_std::FutureExt;
          $crate::select!{@INNER {$($collect)* $name = $future.fuse()=>$result, } $($r)*}
      }
  };
  (@INNER {$($collect:tt)*} $name:pat = $future:expr => $result:expr , $($r:tt)*) => {
      {
          #[allow(unused_imports)]
          use $crate::async_compat::task::async_std::FutureExt;
          $crate::select!{@INNER {$($collect)* $name = $future.fuse()=>$result, } $($r)*}
      }
  };
  (@INNER {$($collect:tt)*} $name:pat = $future:expr => $result:block  $($r:tt)*) => {
      {
          #[allow(unused_imports)]
          use $crate::async_compat::task::async_std::FutureExt;
          $crate::select!{@INNER {$($collect)* $name = $future.fuse()=>$result, } $($r)*}
      }
  };
  (@INNER {$($collect:tt)*} complete => $result:expr , $($r:tt)*) =>{
      {

          $crate::select!{@INNER {$($collect)* complete =>$result, } $($r)*}
      }
  };
  (@INNER {$($collect:tt)*} complete => $result:block $($r:tt)*) =>{
      {

          $crate::select!{@INNER {$($collect)* complete =>$result, } $($r)*}
      }
  };
    (@INNER {$($collect:tt)*}  default => $result:expr $(,)?) =>{
      {

          $crate::select!{@INNER {$($collect)*  default =>$result, }}
      }
  };
   (@INNER {$($collect:tt)*}) => {
       $crate::async_compat::task::async_std::futures::select! {
           $($collect)*
       }
   };
    ($($tokens:tt)*) => {
      $crate::select!{@INNER {} $($tokens)*}
   };
}
