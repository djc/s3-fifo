use std::borrow::Borrow;
use std::collections::VecDeque;
use std::hash::Hash;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering::SeqCst;

pub struct S3Fifo<K, V> {
    small: VecDeque<Item<K, V>>,
    main: VecDeque<Item<K, V>>,
    ghost: VecDeque<K>,
}

impl<K: Hash + Eq, V> S3Fifo<K, V> {
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            small: VecDeque::with_capacity(capacity / 10),
            main: VecDeque::with_capacity(capacity / 10 * 9),
            ghost: VecDeque::with_capacity(capacity / 10 * 9),
        }
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq + ?Sized,
    {
        let item = match self.small.iter().find(|item| item.key.borrow() == key) {
            Some(item) => item,
            None => match self.main.iter().find(|item| item.key.borrow() == key) {
                Some(item) => item,
                None => return None,
            },
        };

        let inc = |freq: u8| Some(Ord::min(freq + 1, 3));
        // If this fails, this might pushed into the `ghost` queue too soon.
        let _ = item.freq.fetch_update(SeqCst, SeqCst, inc);
        Some(&item.value)
    }

    pub fn insert(&mut self, key: K, value: V) {
        if self.small.len() == self.small.capacity() {
            self.evict_small();
        }

        // Does the new entry take its `freq` from the ghost queue?
        match self.ghost.contains(&key) {
            true => &mut self.main,
            false => &mut self.small,
        }
        .push_front(Item::new(key, value));
    }

    fn evict_small(&mut self) {
        while let Some(tail) = self.small.pop_back() {
            if tail.freq.load(SeqCst) > 1 {
                if self.main.len() == self.main.capacity() {
                    self.evict_main();
                }

                self.main.push_front(tail);
                continue;
            }

            self.ghost.push_front(tail.key);
            break;
        }
    }

    fn evict_main(&mut self) {
        while let Some(tail) = self.main.pop_back() {
            let dec = |freq: u8| Some(freq.saturating_sub(1));
            let freq = match tail.freq.fetch_update(SeqCst, SeqCst, dec) {
                Ok(prev) => prev,
                // If the decrement failed, we'll insert this at the head of the queue for one
                // more round, this should be okay if it happens rarely.
                Err(prev) => prev,
            };

            if freq > 0 {
                self.main.push_front(tail);
            } else {
                break;
            }
        }
    }

    pub fn len(&self) -> usize {
        self.small.len() + self.main.len()
    }
}

struct Item<K, V> {
    freq: AtomicU8,
    key: K,
    value: V,
}

impl<K, V> Item<K, V> {
    fn new(key: K, value: V) -> Self {
        Self {
            freq: AtomicU8::default(),
            key,
            value,
        }
    }
}
