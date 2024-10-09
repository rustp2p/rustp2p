use bytes::BytesMut;
use crossbeam_queue::ArrayQueue;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

#[derive(Clone)]
pub struct BufferPool<T> {
    queue: Arc<ArrayQueue<T>>,
    buf_capacity: usize,
}
impl<T> BufferPool<T> {
    pub fn new(capacity: usize, buf_capacity: usize) -> Self {
        Self {
            queue: Arc::new(ArrayQueue::new(capacity)),
            buf_capacity,
        }
    }
}
impl<T: Allocatable> BufferPool<T> {
    pub fn alloc(&self) -> Block<T> {
        if let Some(mut data) = self.queue.pop() {
            data.clear();
            Block::new(self.queue.clone(), data)
        } else {
            Block::new(self.queue.clone(), T::alloc(self.buf_capacity))
        }
    }
}

pub struct Block<T> {
    queue: Arc<ArrayQueue<T>>,
    data: std::mem::ManuallyDrop<T>,
}
impl<T> Block<T> {
    pub fn new(queue: Arc<ArrayQueue<T>>, data: T) -> Self {
        Self {
            queue,
            data: std::mem::ManuallyDrop::new(data),
        }
    }
}
impl<T> Drop for Block<T> {
    fn drop(&mut self) {
        let data = unsafe { std::mem::ManuallyDrop::take(&mut self.data) };
        let _ = self.queue.push(data);
    }
}
impl<T> Deref for Block<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        self.data.deref()
    }
}
impl<T> DerefMut for Block<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data.deref_mut()
    }
}

pub trait Allocatable {
    fn alloc(capacity: usize) -> Self;
    fn clear(&mut self);
}

impl Allocatable for BytesMut {
    fn alloc(capacity: usize) -> Self {
        BytesMut::with_capacity(capacity)
    }

    fn clear(&mut self) {
        self.clear();
    }
}
