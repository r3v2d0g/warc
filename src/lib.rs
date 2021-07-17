use std::cell::Cell;
use std::cmp::{self, Eq, Ord, PartialEq, PartialOrd};
use std::fmt::{self, Debug, Display, Formatter};
use std::marker::PhantomData;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::atomic::{self, AtomicUsize, Ordering};

const DEFAULT_WEIGHT: usize = 1 << 16;
const ADD_WEIGHT: usize = DEFAULT_WEIGHT - 1;

pub struct Warc<T: ?Sized> {
    local: Cell<usize>,
    inner: NonNull<Inner<T>>,
    _chck: PhantomData<Inner<T>>,
}

struct Inner<T: ?Sized> {
    global: AtomicUsize,
    value: T,
}

impl<T> Warc<T> {
    pub fn new(value: T) -> Self {
        let inner = Box::new(Inner {
            global: AtomicUsize::new(DEFAULT_WEIGHT),
            value,
        });

        Warc {
            local: Cell::new(DEFAULT_WEIGHT),
            inner: Box::leak(inner).into(),
            _chck: PhantomData,
        }
    }
}

impl<T: ?Sized> Warc<T> {
    fn inner(&self) -> &Inner<T> {
        unsafe { self.inner.as_ref() }
    }

    #[cfg(test)]
    fn local(&self) -> usize {
        self.local.get()
    }

    #[cfg(test)]
    fn global(&self) -> usize {
        self.inner().global.load(Ordering::Acquire)
    }
}

unsafe impl<T: ?Sized + Send + Sync> Send for Warc<T> {}

impl<T: ?Sized> Drop for Warc<T> {
    fn drop(&mut self) {
        if self.inner().global.fetch_sub(self.local.get(), Ordering::Release) != 0 {
            return;
        }

        atomic::fence(Ordering::Acquire);

        drop(unsafe { Box::from_raw(self.inner.as_ptr()) })
    }
}

impl<T: ?Sized> Clone for Warc<T> {
    fn clone(&self) -> Self {
        let mut local = self.local.get();
        if local == 1 {
            let inner = self.inner();
            let mut current_global = inner.global.load(Ordering::Acquire);
            let mut new_global;
            loop {
                if current_global != usize::MAX {
                    new_global = current_global + ADD_WEIGHT;
                } else {
                    panic!("global weight is too high");
                }

                match inner.global.compare_exchange_weak(current_global, new_global, Ordering::AcqRel, Ordering::Acquire) {
                    Ok(_) => break,
                    Err(global) => current_global = global,
                }
            }

            local += new_global - current_global;
        }

        local >>= 1;
        self.local.set(local);

        Warc {
            local: Cell::new(local),
            inner: self.inner,
            _chck: PhantomData,
        }
    }
}

impl<T: Default> Default for Warc<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: ?Sized> AsRef<T> for Warc<T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}

impl<T: ?Sized> Deref for Warc<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner().value
    }
}

impl<T: Debug + ?Sized> Debug for Warc<T> {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        self.inner().value.fmt(fmt)
    }
}

impl<T: Display + ?Sized> Display for Warc<T> {
    fn fmt(&self, fmt: &mut Formatter) -> fmt::Result {
        self.inner().value.fmt(fmt)
    }
}

impl<T: Eq + ?Sized> Eq for Warc<T> {}

impl<T: PartialEq + ?Sized> PartialEq for Warc<T> {
    fn eq(&self, other: &Self) -> bool {
        self.inner().value.eq(&other.inner().value)
    }
}

impl<T: Ord + ?Sized> Ord for Warc<T> {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.inner().value.cmp(&other.inner().value)
    }
}

impl<T: PartialOrd + ?Sized> PartialOrd for Warc<T> {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        self.inner().value.partial_cmp(&other.inner().value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local() {
        let warc = Warc::new(());
        assert_eq!(warc.local(), DEFAULT_WEIGHT);
        assert_eq!(warc.global(), DEFAULT_WEIGHT);

        let warcp = warc.clone();
        assert_eq!(warcp.local(), DEFAULT_WEIGHT >> 1);
        assert_eq!(warc.local(), DEFAULT_WEIGHT >> 1);
        assert_eq!(warc.global(), DEFAULT_WEIGHT);

        drop(warcp);
        assert_eq!(warc.local(), DEFAULT_WEIGHT >> 1);
        assert_eq!(warc.global(), DEFAULT_WEIGHT >> 1);
    }

    #[test]
    fn global() {
        let warc = Warc::new(());
        let mut clones = Vec::with_capacity(16);

        for i in 0..16 {
            assert_eq!(warc.local(), DEFAULT_WEIGHT >> i);
            assert_eq!(warc.global(), DEFAULT_WEIGHT);

            let warcp = warc.clone();
            assert_eq!(warcp.local(), DEFAULT_WEIGHT >> (i + 1));
            assert_eq!(warc.local(), DEFAULT_WEIGHT >> (i + 1));

            clones.push(warcp);
        }

        assert_eq!(warc.local(), 1);

        let warcp = warc.clone();
        assert_eq!(warcp.local(), DEFAULT_WEIGHT >> 1);
        assert_eq!(warc.local(), DEFAULT_WEIGHT >> 1);
        assert_eq!(warc.global(), (DEFAULT_WEIGHT << 1) - 1);

        assert_eq!(
            warc.global(),
            warc.local() + warcp.local() + clones.iter().map(Warc::local).sum::<usize>(),
        );

        clones.clear();
        assert_eq!(warc.global(), warc.local() + warcp.local());
    }
}
