use bytes::BytesMut;
use crossbeam_queue::ArrayQueue;
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone)]
pub struct RecycleBuf {
    queue: Arc<ArrayQueue<BytesMut>>,
    recycle_range: Range<usize>,
}
impl RecycleBuf {
    pub fn new(cap: usize, recycle_range: Range<usize>) -> Self {
        let queue = Arc::new(ArrayQueue::new(cap));
        Self {
            queue,
            recycle_range,
        }
    }
    pub fn push(&self, mut buf: BytesMut) {
        if self.recycle_range.contains(&buf.capacity()) {
            buf.clear();
            let _ = self.queue.push(buf);
        }
    }
    pub fn pop(&self) -> Option<BytesMut> {
        self.queue.pop()
    }
}
