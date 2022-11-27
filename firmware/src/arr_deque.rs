use std::mem::MaybeUninit;

pub struct ArrDeque<T, const N: usize> {
    full: bool,
    start: usize,
    end: usize,
    arr: [MaybeUninit<T>; N],
}

impl<T, const N: usize> ArrDeque<T, N> {
    pub const fn new() -> ArrDeque<T, N> {
        ArrDeque {
            full: false,
            start: 0,
            end: 0,
            // https://doc.rust-lang.org/stable/std/mem/union.MaybeUninit.html#initializing-an-array-element-by-element
            arr: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    pub fn len(&self) -> usize {
        if self.full {
            N
        } else if self.end >= self.start {
            self.end - self.start
        } else {
            N - self.start + self.end
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.full && self.start == self.end
    }

    pub fn overwriting_push_back(&mut self, value: T) {
        if self.full {
            self.pop_front();
        }

        self.arr[self.end].write(value);
        if self.end < N - 1 {
            self.end += 1;
        } else {
            self.end = 0;
        }
        self.full = self.start == self.end;
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }

        let pos = self.start;

        if self.start < N - 1 {
            self.start += 1;
        } else {
            self.start = 0;
        }
        self.full = false;

        let value = unsafe { self.arr[pos].assume_init_read() };
        Some(value)
    }

    pub fn iter(&self) -> Iter<T, N> {
        Iter::new(self)
    }
}

impl<T, const N: usize> Drop for ArrDeque<T, N> {
    fn drop(&mut self) {
        while let Some(_) = self.pop_front() {}
    }
}

pub struct Iter<'a, T, const N: usize> {
    deque: &'a ArrDeque<T, N>,
    first: bool,
    position: usize,
}

impl<'a, T, const N: usize> Iter<'a, T, N> {
    fn new(deque: &'a ArrDeque<T, N>) -> Self {
        Iter {
            deque: &deque,
            first: !deque.is_empty(),
            position: deque.start,
        }
    }
}

impl<'a, T, const N: usize> Iterator for Iter<'a, T, N> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        let pos = self.position;

        if !self.first && pos == self.deque.end {
            return None;
        }
        self.first = false;

        if self.position < N - 1 {
            self.position += 1;
        } else {
            self.position = 0;
        }

        let value = unsafe { self.deque.arr[pos].assume_init_ref() };
        Some(value)
    }
}

#[test]
pub fn test_arr_deque() {
    let mut deque: ArrDeque<u8, 5> = ArrDeque::new();
    for _ in 0..3 {
        for _ in 0..3 {
            for i in 0..7 {
                deque.overwriting_push_back(i);
            }
            let mut iter = deque.iter();
            for i in 2..7 {
                assert_eq!(iter.next().cloned(), Some(i));
            }
            assert_eq!(iter.next(), None);
        }
        for i in 2..7 {
            assert_eq!(deque.pop_front(), Some(i));
        }
        assert_eq!(deque.pop_front(), None);
    }
}
