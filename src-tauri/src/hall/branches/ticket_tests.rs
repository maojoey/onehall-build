use super::tickets::TicketStore;
use serde_json::json;
use std::time::Duration;
#[tokio::test]
async fn tickets_are_branch_revision_scoped_one_use_and_hide_upstream_capability() {
    let store = TicketStore::new(Duration::from_secs(60), 2);
    let input = json!({"ticket":"0123456789abcdef0123456789abcdef","url":"http://evil.example/x","filename":"a.html","expiresAt":4102444800i64});
    let issued = store.mint("branch-a", 3, &input).await.unwrap();
    assert_eq!(issued["filename"], "a.html");
    let cap = issued["ticket"].as_str().unwrap();
    assert_ne!(cap, "0123456789abcdef0123456789abcdef");
    assert_eq!(issued["url"], format!("/branch-download/{cap}"));
    assert!(!issued.to_string().contains("evil.example"));
    assert!(store.take(cap, Some("branch-b")).await.is_none());
    let ticket = store.take(cap, Some("branch-a")).await.unwrap();
    assert_eq!(ticket.branch_id, "branch-a");
    assert_eq!(ticket.revision, 3);
    assert_eq!(ticket.upstream, "0123456789abcdef0123456789abcdef");
    assert!(store.take(cap, None).await.is_none());
}
#[tokio::test]
async fn tickets_expire_and_capacity_and_untrusted_tokens_are_bounded() {
    let store = TicketStore::new(Duration::from_millis(5), 1);
    for token in ["../escape", "https://evil.test", "0123?token=bad"] {
        assert!(store
            .mint(
                "a",
                1,
                &json!({"ticket":token,"filename":"a","expiresAt":4102444800i64})
            )
            .await
            .is_err());
    }
    let input = json!({"ticket":"0123456789abcdef0123456789abcdef","filename":"a","expiresAt":4102444800i64});
    let issued = store.mint("a", 1, &input).await.unwrap();
    assert!(store.mint("b", 1, &input).await.is_err());
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(store
        .take(issued["ticket"].as_str().unwrap(), None)
        .await
        .is_none());
    assert!(store.mint("b", 1, &input).await.is_ok());
}
