use std::{
    collections::VecDeque,
    sync::{Condvar, Mutex},
};

#[derive(Default)]
pub struct WorkQueue<T> {
    data: Mutex<VecDeque<T>>,
    cv: Condvar,
}

impl<T> WorkQueue<T> {
    pub fn new() -> Self {
        Self {
            data: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
        }
    }

    pub fn push(&self, work_item: T) {
        let mut data = self.data.lock().unwrap();
        data.push_back(work_item);
        self.cv.notify_all();
    }

    pub fn wait_for_work(&self) -> T {
        let mut data = self.data.lock().unwrap();

        // wait for the notification if the queue is empty
        while data.is_empty() {
            data = self.cv.wait(data).unwrap();
        }

        data.pop_front().unwrap()
    }
}

#[cfg(test)]
#[test]
fn test_workqueue() {
    use std::{sync::Arc, thread};

    let queue = Arc::new(WorkQueue::<i32>::new());
    let queue2 = queue.clone();
    let _result = Arc::new(Mutex::new(0));

    let producer = thread::spawn(move || {
        queue.push(5i32);
    });

    let consumer = thread::spawn(move || queue2.wait_for_work());

    producer.join();
    let result = consumer.join().unwrap();

    assert_eq!(result, 5i32);
}
