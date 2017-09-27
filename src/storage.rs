//! Ring buffer implementation, that does immutable reads.

use std::any::TypeId;
use std::fmt;
use std::ops::{Index, IndexMut};

/// Ringbuffer errors
pub enum RBError<'a, T: 'a> {
    /// If a writer tries to write more data than the max size of the ringbuffer, in a single call
    TooLargeWrite,
    /// If a reader is more than the entire ringbuffer behind in reading, this will be returned.
    /// Contains the data that could be salvaged, and the amount of data that was lost.
    LostData(StorageIterator<'a, T>, usize),
    /// If attempting to use a reader for a different data type than the storage contains.
    InvalidReader,
}

impl<'a, T: 'a> fmt::Debug for RBError<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            RBError::TooLargeWrite => write!(f, "TooLargeWrite"),
            RBError::InvalidReader => write!(f, "InvalidReader"),
            RBError::LostData(..) => write!(f, "LostData"),
        }
    }
}

impl<'a, T: 'a> PartialEq for RBError<'a, T> {
    fn eq(&self, other: &RBError<'a, T>) -> bool {
        match (self, other) {
            (&RBError::TooLargeWrite, &RBError::TooLargeWrite) => true,
            (&RBError::InvalidReader, &RBError::InvalidReader) => true,
            (&RBError::LostData(..), &RBError::LostData(..)) => true,
            _ => false,
        }
    }
}

/// The reader id is used by readers to tell the storage where the last read ended.
#[derive(Hash, PartialEq, Copy, Clone, Debug)]
pub struct ReaderId {
    t: TypeId,
    id: u32,
    read_index: usize,
    written: usize,
}

impl ReaderId {
    /// Create a new reader id
    pub fn new(t: TypeId, id: u32, reader_index: usize, written: usize) -> ReaderId {
        ReaderId {
            t: t,
            id: id,
            read_index: reader_index,
            written: written,
        }
    }
}

/// Ring buffer, holding data of type `T`
pub struct RingBufferStorage<T> {
    pub(crate) data: Vec<T>,
    write_index: usize,
    max_size: usize,
    written: usize,
    next_reader_id: u32,
    reset_written: usize,
}

impl<T: 'static> RingBufferStorage<T> {
    /// Create a new ring buffer with the given max size.
    pub fn new(size: usize) -> Self {
        RingBufferStorage {
            data: Vec::with_capacity(size),
            write_index: 0,
            max_size: size,
            written: 0,
            next_reader_id: 1,
            reset_written: size * 1000,
        }
    }

    /// Write a set of data into the ringbuffer.
    pub fn write(&mut self, data: &mut Vec<T>) -> Result<(), RBError<T>> {
        if data.len() == 0 {
            return Ok(());
        }
        if data.len() > self.max_size {
            return Err(RBError::TooLargeWrite);
        }
        for d in data.drain(0..) {
            self.write_single(d);
        }
        Ok(())
    }

    /// Write a single data point into the ringbuffer.
    pub fn write_single(&mut self, data: T) {
        let mut write_index = self.write_index;
        if write_index == self.data.len() {
            self.data.push(data);
        } else {
            self.data[write_index] = data;
        }
        write_index += 1;
        if write_index >= self.max_size {
            write_index = 0;
        }
        self.write_index = write_index;
        self.written += 1;
        if self.written > self.reset_written {
            self.written = 0;
        }
    }

    /// Create a new reader id for this ringbuffer.
    pub fn new_reader_id(&mut self) -> ReaderId {
        let reader_id = ReaderId::new(
            TypeId::of::<T>(),
            self.next_reader_id,
            self.write_index,
            self.written,
        );
        self.next_reader_id += 1;
        reader_id
    }

    /// Read data from the ringbuffer, starting where the last read ended, and up to where the last
    /// data was written.
    pub fn read(&self, reader_id: &mut ReaderId) -> Result<StorageIterator<T>, RBError<T>> {
        if reader_id.t != TypeId::of::<T>() {
            return Err(RBError::InvalidReader);
        }
        let num_written = if self.written < reader_id.written {
            self.written + (self.reset_written - reader_id.written)
        } else {
            self.written - reader_id.written
        };

        let read_index = reader_id.read_index;
        reader_id.read_index = self.write_index;
        reader_id.written = self.written;

        if num_written > self.max_size {
            Err(RBError::LostData(
                StorageIterator {
                    storage: &self,
                    current: self.write_index,
                    end: self.write_index,
                    started: false,
                },
                num_written - self.max_size,
            ))
        } else {
            Ok(StorageIterator {
                storage: &self,
                current: read_index,
                end: self.write_index,
                // handle corner case no data to read
                started: num_written == 0,
            })
        }
    }
}

/// Iterator over a slice of data in `RingbufferStorage`.
pub struct StorageIterator<'a, T: 'a> {
    storage: &'a RingBufferStorage<T>,
    current: usize,
    end: usize,
    // needed when we should read the whole buffer, because then current == end for the first value
    // needs special handling for empty iterator, needs to be forced to true for that corner case
    started: bool,
}

impl<'a, T> Iterator for StorageIterator<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        if self.started && self.current == self.end {
            None
        } else {
            self.started = true;
            let t = &self.storage[self.current];
            self.current += 1;
            if self.current == self.storage.data.len() && self.end != self.storage.data.len() {
                self.current = 0;
            }
            Some(t)
        }
    }
}

impl<T> Index<usize> for RingBufferStorage<T> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.data[index]
    }
}

impl<T> IndexMut<usize> for RingBufferStorage<T> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.data[index]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::TypeId;

    #[derive(Debug, Clone, PartialEq)]
    struct Test {
        pub id: u32,
    }

    #[derive(Debug, Clone, PartialEq)]
    struct Test2 {
        pub id: u32,
    }

    #[test]
    fn test_empty_write() {
        let mut buffer = RingBufferStorage::<Test>::new(10);
        let r = buffer.write(&mut vec![]);
        assert!(r.is_ok());
    }

    #[test]
    fn test_too_large_write() {
        let mut buffer = RingBufferStorage::<Test>::new(10);
        let r = buffer.write(&mut events(15));
        assert!(r.is_err());
        assert_eq!(RBError::TooLargeWrite, r.unwrap_err());
    }

    #[test]
    fn test_invalid_reader() {
        let buffer = RingBufferStorage::<Test>::new(10);
        let mut reader_id = ReaderId::new(TypeId::of::<Test2>(), 4, 0, 0);
        let r = buffer.read(&mut reader_id);
        assert!(r.is_err());
        match r {
            Err(RBError::InvalidReader) => (),
            _ => panic!(),
        }
    }

    #[test]
    fn test_empty_read() {
        let mut buffer = RingBufferStorage::<Test>::new(10);
        let mut reader_id = buffer.new_reader_id();
        assert_eq!(
            Vec::<Test>::default(),
            buffer
                .read(&mut reader_id)
                .unwrap()
                .cloned()
                .collect::<Vec<Test>>()
        );
    }

    #[test]
    fn test_empty_read_write_before_id() {
        let mut buffer = RingBufferStorage::<Test>::new(10);
        assert_eq!(Ok(()), buffer.write(&mut events(2)));
        let mut reader_id = buffer.new_reader_id();
        assert_eq!(
            Vec::<Test>::default(),
            buffer
                .read(&mut reader_id)
                .unwrap()
                .cloned()
                .collect::<Vec<Test>>()
        );
    }

    #[test]
    fn test_read() {
        let mut buffer = RingBufferStorage::<Test>::new(10);
        let mut reader_id = buffer.new_reader_id();
        assert_eq!(Ok(()), buffer.write(&mut events(2)));
        assert_eq!(
            vec![Test { id: 0 }, Test { id: 1 }],
            buffer
                .read(&mut reader_id)
                .unwrap()
                .cloned()
                .collect::<Vec<Test>>()
        );
    }

    #[test]
    fn test_write_overflow() {
        let mut buffer = RingBufferStorage::<Test>::new(3);
        let mut reader_id = buffer.new_reader_id();
        assert_eq!(Ok(()), buffer.write(&mut events(2)));
        assert_eq!(Ok(()), buffer.write(&mut events(2)));
        let r = buffer.read(&mut reader_id);
        assert!(r.is_err());
        let (has_lost_data, lost_data, lost_size) = match r {
            Err(RBError::LostData(d, s)) => (true, d.cloned().collect::<Vec<_>>(), s),
            _ => (false, vec![], 0),
        };
        assert!(has_lost_data);
        // we wrote 4 data points into a buffer of size 3, that means we've lost 1 data point
        assert_eq!(1, lost_size);
        // we wrote 0,1,0,1, we will be able to salvage the last 3 data points, since the buffer is
        // of size 3
        assert_eq!(
            vec![Test { id: 1 }, Test { id: 0 }, Test { id: 1 }],
            lost_data
        );
    }

    fn events(n: u32) -> Vec<Test> {
        (0..n).map(|i| Test { id: i }).collect::<Vec<_>>()
    }
}
