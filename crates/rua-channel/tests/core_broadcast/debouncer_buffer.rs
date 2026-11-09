use std::time::Duration;

use rua_channel::core::BufferDebouncer;

#[tokio::test]
async fn push_three_same_key_returns_merged_buffer() {
    let d = BufferDebouncer::<String, u32>::new(Duration::from_millis(100));

    let d1 = d.clone();
    let handle = tokio::spawn(async move { d1.push("k".into(), 1).await });

    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(d.push("k".into(), 2).await, None);
    assert_eq!(d.push("k".into(), 3).await, None);

    let result = handle.await.unwrap();
    assert_eq!(result, Some(vec![1, 2, 3]));
}

#[tokio::test]
async fn different_keys_are_independent() {
    let d = BufferDebouncer::<String, u32>::new(Duration::from_millis(100));

    let d1 = d.clone();
    let h_a = tokio::spawn(async move { d1.push("a".into(), 10).await });

    let d2 = d.clone();
    let h_b = tokio::spawn(async move { d2.push("b".into(), 20).await });

    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(d.push("a".into(), 11).await, None);
    assert_eq!(d.push("b".into(), 21).await, None);

    let ra = h_a.await.unwrap();
    let rb = h_b.await.unwrap();
    assert_eq!(ra, Some(vec![10, 11]));
    assert_eq!(rb, Some(vec![20, 21]));
}

#[tokio::test]
async fn new_window_after_previous_drains() {
    let d = BufferDebouncer::<String, u32>::new(Duration::from_millis(50));

    let d1 = d.clone();
    let h1 = tokio::spawn(async move { d1.push("k".into(), 1).await });
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(d.push("k".into(), 2).await, None);

    let r1 = h1.await.unwrap();
    assert_eq!(r1, Some(vec![1, 2]));

    let d2 = d.clone();
    let h2 = tokio::spawn(async move { d2.push("k".into(), 3).await });
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(d.push("k".into(), 4).await, None);

    let r2 = h2.await.unwrap();
    assert_eq!(r2, Some(vec![3, 4]));
}

#[tokio::test]
async fn single_push_returns_singleton_vec() {
    let d = BufferDebouncer::<String, &str>::new(Duration::from_millis(50));

    let result = d.push("k".into(), "only").await;
    assert_eq!(result, Some(vec!["only"]));
}

#[tokio::test]
async fn cancelled_first_push_does_not_poison_future_windows() {
    let d = BufferDebouncer::<String, u32>::new(Duration::from_millis(25));

    let d1 = d.clone();
    let first = tokio::spawn(async move { d1.push("k".into(), 1).await });
    tokio::time::sleep(Duration::from_millis(5)).await;
    first.abort();
    let _ = first.await;

    tokio::time::sleep(Duration::from_millis(50)).await;

    let d2 = d.clone();
    let second = tokio::spawn(async move { d2.push("k".into(), 2).await });
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert_eq!(d.push("k".into(), 3).await, None);

    assert_eq!(second.await.unwrap(), Some(vec![2, 3]));
}
