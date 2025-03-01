use std::sync::mpsc::{Receiver, TryRecvError};

pub(crate) struct PeekableReceiver<T> {
    rx: Receiver<T>,
    peeked: Option<T>,
}

impl<T> PeekableReceiver<T> {
    pub(crate) fn new(rx: Receiver<T>) -> Self {
        Self { rx, peeked: None }
    }

    pub(crate) fn is_empty(&mut self) -> bool {
        self.peek().is_none()
    }

    pub(crate) fn peek(&mut self) -> Option<&T> {
        if self.peeked.is_some() {
            self.peeked.as_ref()
        } else {
            match self.rx.try_recv() {
                Ok(value) => {
                    self.peeked = Some(value);
                    self.peeked.as_ref()
                }
                Err(_) => None,
            }
        }
    }

    pub(crate) fn try_recv(&mut self) -> Result<T, TryRecvError> {
        if let Some(value) = self.peeked.take() {
            Ok(value)
        } else {
            self.rx.try_recv()
        }
    }
}
