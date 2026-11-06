//! Valence-injected API ops: list / page / counts / mark (happy + sad).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use common::{as_system, as_user, setup_shared_db, TEST_USER_A, TEST_USER_B};
use uf_notifications_api::{
    list_for_user, mark_all_read_for_user, mark_read_for_user, mark_unread_for_user,
    notification_count_for_user, notifications_page_for_user, today_count_for_user,
    unread_count_for_user, unread_page_for_user, unread_preview_for_user, NotificationReadFilter,
};
use uf_notifications_core::{send_notification, SendNotification};
use uuid::Uuid;
use valence::RecordId;

fn sample(user: &str, title: &str) -> SendNotification {
    SendNotification {
        user_id: RecordId::new("user", user),
        kind: "test".into(),
        title: title.into(),
        message: format!("body for {title}"),
        url: Some("/notifications".into()),
        data_json: None,
        subject: None,
    }
}

async fn mint(system: &valence::Valence, user: &str, title: &str) -> Uuid {
    let dto = send_notification(sample(user, title), system)
        .await
        .expect("system mint");
    dto.notification_id
}

#[tokio::test]
async fn list_and_counts_happy_peer_isolated_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let peer = as_user(&base, TEST_USER_B);
    let owner_id = RecordId::new("user", TEST_USER_A);

    mint(&system, TEST_USER_A, "Alpha").await;
    mint(&system, TEST_USER_A, "Beta").await;
    mint(&system, TEST_USER_B, "PeerOnly").await;

    let listed = list_for_user(&owner, owner_id.clone()).await.expect("list");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|n| n.title == "Alpha"));
    assert!(listed.iter().any(|n| n.title == "Beta"));
    assert!(!listed.iter().any(|n| n.title == "PeerOnly"));

    assert_eq!(
        unread_count_for_user(&owner, owner_id.clone())
            .await
            .expect("unread"),
        2
    );
    assert_eq!(
        notification_count_for_user(&owner, owner_id.clone())
            .await
            .expect("total"),
        2
    );
    let today = today_count_for_user(&owner, owner_id.clone())
        .await
        .expect("today");
    assert_eq!(today, 2);

    let peer_list = list_for_user(&peer, owner_id.clone())
        .await
        .expect("peer list as owner id still scoped by actor privacy");
    // Peer Valence with owner's RecordId in the query still applies actor privacy —
    // rows owned by A are not visible to B's actor.
    assert!(
        peer_list.is_empty(),
        "peer actor must not see owner rows, got {peer_list:?}"
    );
}

#[tokio::test]
async fn unread_preview_and_page_happy_empty_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let owner_id = RecordId::new("user", TEST_USER_A);

    let empty = unread_preview_for_user(&owner, owner_id.clone())
        .await
        .expect("empty preview");
    assert_eq!(empty.len(), 0);

    let empty_page = unread_page_for_user(&owner, owner_id.clone(), 0, 5)
        .await
        .expect("empty page");
    assert_eq!(empty_page.items.len(), 0);
    assert!(!empty_page.has_more);
    assert_eq!(empty_page.total_count, Some(0));

    for i in 0..12 {
        mint(&system, TEST_USER_A, &format!("Unread-{i}")).await;
    }

    let preview = unread_preview_for_user(&owner, owner_id.clone())
        .await
        .expect("preview");
    assert_eq!(preview.len(), 10);
    assert!(preview.iter().all(|n| !n.is_read));

    let page0 = unread_page_for_user(&owner, owner_id.clone(), 0, 5)
        .await
        .expect("page0");
    assert_eq!(page0.items.len(), 5);
    assert!(page0.has_more);
    assert_eq!(page0.total_count, Some(12));

    let page1 = unread_page_for_user(&owner, owner_id.clone(), 5, 5)
        .await
        .expect("page1");
    assert_eq!(page1.items.len(), 5);
    assert!(page1.has_more);
    assert!(page1.total_count.is_none());

    let page2 = unread_page_for_user(&owner, owner_id, 10, 5)
        .await
        .expect("page2");
    assert_eq!(page2.items.len(), 2);
    assert!(!page2.has_more);
}

#[tokio::test]
async fn notifications_page_filter_search_happy_and_empty_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let owner_id = RecordId::new("user", TEST_USER_A);

    let read_id = mint(&system, TEST_USER_A, "KeepMe").await;
    mint(&system, TEST_USER_A, "DropMe").await;
    mark_read_for_user(&owner, read_id)
        .await
        .expect("mark KeepMe read");

    let all = notifications_page_for_user(
        &owner,
        owner_id.clone(),
        0,
        20,
        None,
        NotificationReadFilter::All,
    )
    .await
    .expect("all");
    assert_eq!(all.items.len(), 2);

    let unread = notifications_page_for_user(
        &owner,
        owner_id.clone(),
        0,
        20,
        None,
        NotificationReadFilter::Unread,
    )
    .await
    .expect("unread");
    assert_eq!(unread.items.len(), 1);
    assert_eq!(unread.items[0].title, "DropMe");

    let read = notifications_page_for_user(
        &owner,
        owner_id.clone(),
        0,
        20,
        None,
        NotificationReadFilter::Read,
    )
    .await
    .expect("read");
    assert_eq!(read.items.len(), 1);
    assert_eq!(read.items[0].title, "KeepMe");

    let search = notifications_page_for_user(
        &owner,
        owner_id.clone(),
        0,
        20,
        Some("KeepMe".into()),
        NotificationReadFilter::All,
    )
    .await
    .expect("search");
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].title, "KeepMe");

    let miss = notifications_page_for_user(
        &owner,
        owner_id,
        0,
        20,
        Some("zz-no-match".into()),
        NotificationReadFilter::All,
    )
    .await
    .expect("miss");
    assert_eq!(miss.items.len(), 0);
    assert_eq!(miss.total_count, Some(0));
}

#[tokio::test]
async fn mark_read_unread_all_happy_and_not_found_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let owner_id = RecordId::new("user", TEST_USER_A);

    let id_a = mint(&system, TEST_USER_A, "A").await;
    let id_b = mint(&system, TEST_USER_A, "B").await;

    mark_read_for_user(&owner, id_a).await.expect("mark a");
    let unread = unread_count_for_user(&owner, owner_id.clone())
        .await
        .expect("unread after one");
    assert_eq!(unread, 1);

    mark_read_for_user(&owner, id_a)
        .await
        .expect("idempotent re-mark");

    mark_unread_for_user(&owner, id_a)
        .await
        .expect("mark unread");
    assert_eq!(
        unread_count_for_user(&owner, owner_id.clone())
            .await
            .expect("unread restored"),
        2
    );

    let marked = mark_all_read_for_user(&owner, owner_id.clone())
        .await
        .expect("mark all");
    assert_eq!(marked, 2);
    assert_eq!(
        unread_count_for_user(&owner, owner_id.clone())
            .await
            .expect("zero unread"),
        0
    );

    let zero = mark_all_read_for_user(&owner, owner_id)
        .await
        .expect("mark all when none");
    assert_eq!(zero, 0);

    // Unknown id is Ok(()) (no enumeration).
    mark_read_for_user(&owner, Uuid::new_v4())
        .await
        .expect("unknown mark read ok");
    mark_unread_for_user(&owner, Uuid::new_v4())
        .await
        .expect("unknown mark unread ok");

    // Peer cannot mark owner's row (privacy → Ok None path).
    let peer = as_user(&base, TEST_USER_B);
    mark_read_for_user(&peer, id_b)
        .await
        .expect("peer mark is silent Ok");
    let owner_again = as_user(&base, TEST_USER_A);
    // id_b was already marked read by mark_all; mint a fresh unread for peer deny check
    let id_c = mint(&system, TEST_USER_A, "C").await;
    mark_read_for_user(&peer, id_c).await.expect("peer silent");
    assert_eq!(
        unread_count_for_user(&owner_again, RecordId::new("user", TEST_USER_A))
            .await
            .expect("still unread for owner"),
        1,
        "peer mark must not commit on owner's row"
    );
}

#[tokio::test]
async fn page_limit_clamp_happy() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let owner_id = RecordId::new("user", TEST_USER_A);

    for i in 0..5 {
        mint(&system, TEST_USER_A, &format!("Clamp-{i}")).await;
    }

    // limit 0 clamps to 1
    let page = unread_page_for_user(&owner, owner_id, 0, 0)
        .await
        .expect("clamp");
    assert_eq!(page.items.len(), 1);
    assert!(page.has_more);
}
