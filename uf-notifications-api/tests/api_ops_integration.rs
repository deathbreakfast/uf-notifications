//! Valence-injected API ops: list / page / counts / mark (happy + sad).

#![allow(missing_docs)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use common::{as_system, as_user, setup_shared_db, TEST_USER_A, TEST_USER_B};
use uf_notifications_api::{
    list_for_user, mark_all_read_for_user, mark_read_for_user, mark_unread_for_user,
    notification_count_for_user, notifications_page_for_request, notifications_page_for_user,
    today_count_for_user, unread_count_for_user, unread_page_for_request, unread_page_for_user,
    unread_preview_for_user, NotificationDto, NotificationReadFilter, Page, PageRequest,
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

/// Page size for the cursor tests.
const PAGE: u32 = 50;

/// Sums the `rows_read` field that `uf_notifications.*` spans record while it is
/// the thread's default subscriber.
#[derive(Default)]
struct RowsRead {
    total: Arc<AtomicUsize>,
    next_id: AtomicU64,
    spans: Mutex<std::collections::HashMap<u64, &'static tracing::Metadata<'static>>>,
    stack: Mutex<Vec<u64>>,
}

struct SumRowsRead<'a>(&'a AtomicUsize);

impl tracing::field::Visit for SumRowsRead<'_> {
    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        if field.name() == "rows_read" {
            self.0
                .fetch_add(usize::try_from(value).unwrap(), Ordering::Relaxed);
        }
    }

    fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
}

impl tracing::Subscriber for RowsRead {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        self.spans.lock().unwrap().insert(id, attrs.metadata());
        tracing::span::Id::from_u64(id)
    }

    fn record(&self, span: &tracing::span::Id, values: &tracing::span::Record<'_>) {
        let ours = self
            .spans
            .lock()
            .unwrap()
            .get(&span.into_u64())
            .is_some_and(|m| m.name().starts_with("uf_notifications."));
        if ours {
            values.record(&mut SumRowsRead(&self.total));
        }
    }

    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}

    fn event(&self, _: &tracing::Event<'_>) {}

    fn enter(&self, span: &tracing::span::Id) {
        self.stack.lock().unwrap().push(span.into_u64());
    }

    fn exit(&self, span: &tracing::span::Id) {
        let mut stack = self.stack.lock().unwrap();
        if let Some(i) = stack.iter().rposition(|&id| id == span.into_u64()) {
            stack.remove(i);
        }
    }

    fn current_span(&self) -> tracing_core::span::Current {
        let top = self.stack.lock().unwrap().last().copied();
        match top.and_then(|id| Some((id, *self.spans.lock().unwrap().get(&id)?))) {
            Some((id, meta)) => {
                tracing_core::span::Current::new(tracing::span::Id::from_u64(id), meta)
            }
            None => tracing_core::span::Current::none(),
        }
    }
}

/// Run `fut` and return its output with the raw rows the page ops read.
async fn rows_read_while<F: std::future::Future>(fut: F) -> (F::Output, usize) {
    let probe = RowsRead::default();
    let total = Arc::clone(&probe.total);
    let _guard = tracing::subscriber::set_default(probe);
    let out = fut.await;
    (out, total.load(Ordering::Relaxed))
}

#[derive(Debug, Clone, Copy)]
enum List {
    Unread,
    Inbox,
}

async fn fetch_page(
    v: &valence::Valence,
    list: List,
    request: &PageRequest,
) -> Page<NotificationDto> {
    let user = RecordId::new("user", TEST_USER_A);
    match list {
        List::Unread => unread_page_for_request(v, user, request)
            .await
            .expect("unread page"),
        List::Inbox => {
            notifications_page_for_request(v, user, request, None, NotificationReadFilter::All)
                .await
                .expect("inbox page")
        }
    }
}

/// What [`page_through`] saw: ids in order, pages read, and how many of
/// those requests carried a cursor.
struct Paged {
    ids: Vec<Uuid>,
    pages: usize,
    cursor_pages: usize,
}

/// Page `list` to the end.
///
/// With `by_cursor`, a request after a page with `next_cursor` sends it with
/// `offset` 0, so a cursor the server ignored would repeat page one. A page
/// without a cursor is followed by an offset request, as the infinite-scroll
/// hook does.
async fn page_through(v: &valence::Valence, list: List, limit: u32, by_cursor: bool) -> Paged {
    let mut ids = Vec::new();
    let mut cursor_pages = 0;
    let mut request = PageRequest::new(0, limit);
    for pages in 1..=500 {
        if request.cursor.is_some() {
            cursor_pages += 1;
        }
        let page = fetch_page(v, list, &request).await;
        ids.extend(page.items.iter().map(|n| n.notification_id));
        if !page.has_more {
            return Paged {
                ids,
                pages,
                cursor_pages,
            };
        }
        request = match page.next_cursor {
            Some(cursor) if by_cursor => PageRequest::new(0, limit).with_cursor(Some(cursor)),
            _ => PageRequest::new(u32::try_from(ids.len()).unwrap(), limit),
        };
    }
    panic!("{list:?}: paging never reached the end");
}

fn page_ids(page: &Page<NotificationDto>) -> Vec<Uuid> {
    page.items.iter().map(|n| n.notification_id).collect()
}

#[tokio::test]
async fn cursor_pages_every_row_once_newest_first_linear_happy() {
    const ROWS: usize = 2_500;
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);

    let mut minted = Vec::with_capacity(ROWS);
    for i in 0..ROWS {
        minted.push(mint(&system, TEST_USER_A, &format!("Many-{i:04}")).await);
        if i % 50 == 0 {
            mint(&system, TEST_USER_B, &format!("Peer-{i:04}")).await;
        }
    }
    let newest_first: Vec<Uuid> = minted.iter().rev().copied().collect();

    for list in [List::Unread, List::Inbox] {
        let (by_cursor, cursor_rows) =
            rows_read_while(page_through(&owner, list, PAGE, true)).await;
        let pages = by_cursor.pages;
        assert_eq!(
            by_cursor.ids.len(),
            ROWS,
            "{list:?}: every row of the user's once, no peer rows"
        );
        assert_eq!(by_cursor.ids, newest_first, "{list:?}: newest first");
        assert_eq!(pages, ROWS / PAGE as usize);
        assert_eq!(
            by_cursor.cursor_pages,
            pages - 1,
            "{list:?}: every page after the first resumes from a cursor"
        );

        let (by_offset, offset_rows) =
            rows_read_while(page_through(&owner, list, PAGE, false)).await;
        assert_eq!(
            by_offset.ids, newest_first,
            "{list:?}: offset paging agrees"
        );

        // Each cursor page reads its rows plus the lookahead row.
        assert!(
            cursor_rows <= 2 * (ROWS + pages * (PAGE as usize + 1)),
            "{list:?}: cursor paging stays linear: {cursor_rows} raw rows over {pages} pages"
        );
        assert!(
            cursor_rows * 10 < offset_rows,
            "{list:?}: cursor paging reads far fewer raw rows ({cursor_rows}) than offset \
             paging ({offset_rows})"
        );
    }
}

#[tokio::test]
async fn cursor_skips_pending_deletion_rows_once_edge() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);

    let mut minted = Vec::new();
    for i in 0..130 {
        minted.push(mint(&system, TEST_USER_A, &format!("Pending-{i:03}")).await);
    }
    // Hidden rows inside pages make the lookahead position unknown, which takes
    // the second-read branch.
    let pending = [3, 49, 50, 51, 120, 129];
    for &i in &pending {
        valence::OwnershipService::mark_pending_deletion(
            "notification",
            &minted[i].to_string(),
            &system,
        )
        .await
        .expect("mark pending");
    }
    let want: Vec<Uuid> = minted
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, _)| !pending.contains(i))
        .map(|(_, id)| *id)
        .collect();

    for limit in [7, 10] {
        for list in [List::Unread, List::Inbox] {
            let got = page_through(&owner, list, limit, true).await;
            assert_eq!(
                got.ids, want,
                "{list:?} limit {limit}: every visible row once"
            );
            assert_eq!(
                got.cursor_pages,
                got.pages - 1,
                "{list:?} limit {limit}: a first page with a hidden row still returns a cursor"
            );
        }
    }
}

#[tokio::test]
async fn stale_foreign_or_garbage_cursor_falls_back_to_offset_sad() {
    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    let peer = as_user(&base, TEST_USER_B);
    let owner_id = RecordId::new("user", TEST_USER_A);

    for i in 0..12 {
        mint(&system, TEST_USER_A, &format!("Row-{i:02}")).await;
    }
    mint(&system, TEST_USER_B, "PeerOnly").await;

    let inbox = |request: PageRequest, query: Option<String>, filter: NotificationReadFilter| {
        let owner = owner.clone();
        let owner_id = owner_id.clone();
        async move {
            notifications_page_for_request(&owner, owner_id, &request, query, filter)
                .await
                .expect("inbox page")
        }
    };

    let unread_cursor = unread_page_for_request(&owner, owner_id.clone(), &PageRequest::new(0, 5))
        .await
        .expect("unread first page")
        .next_cursor
        .expect("unread cursor");
    let first = inbox(PageRequest::new(0, 5), None, NotificationReadFilter::All).await;
    assert_eq!(first.total_count, Some(12));
    let inbox_cursor = first.next_cursor.clone().expect("inbox cursor");

    // The inbox's own cursor is honoured and `offset` is ignored.
    let second = inbox(
        PageRequest::new(0, 5).with_cursor(Some(inbox_cursor.clone())),
        None,
        NotificationReadFilter::All,
    )
    .await;
    let second_by_offset = inbox(PageRequest::new(5, 5), None, NotificationReadFilter::All).await;
    assert_eq!(page_ids(&second), page_ids(&second_by_offset));
    assert!(second.total_count.is_none(), "cursor pages carry no total");

    // Every stale request says offset 0, so the fallback is the first page.
    let stale = [
        (
            "cursor from the unread list",
            PageRequest::new(0, 5).with_cursor(Some(unread_cursor.clone())),
            None,
            NotificationReadFilter::All,
        ),
        (
            "another read filter",
            PageRequest::new(0, 5).with_cursor(Some(inbox_cursor.clone())),
            None,
            NotificationReadFilter::Unread,
        ),
        (
            "another search",
            PageRequest::new(0, 5).with_cursor(Some(inbox_cursor.clone())),
            Some("Row".to_string()),
            NotificationReadFilter::All,
        ),
        (
            "another page size",
            PageRequest::new(0, 6).with_cursor(Some(inbox_cursor.clone())),
            None,
            NotificationReadFilter::All,
        ),
        (
            "garbage",
            PageRequest::new(0, 5).with_cursor(Some("not-a-cursor".to_string())),
            None,
            NotificationReadFilter::All,
        ),
    ];
    for (why, request, query, filter) in stale {
        let limit = request.limit;
        let page = inbox(request, query.clone(), filter).await;
        let fresh = inbox(PageRequest::new(0, limit), query, filter).await;
        assert_eq!(page_ids(&page), page_ids(&fresh), "{why}: pages by offset");
        assert_eq!(page.total_count, Some(12), "{why}: first offset page total");
    }

    // Another user's cursor pages that user's own rows from offset 0.
    let peer_page = unread_page_for_request(
        &peer,
        RecordId::new("user", TEST_USER_B),
        &PageRequest::new(0, 5).with_cursor(Some(unread_cursor)),
    )
    .await
    .expect("peer page");
    assert_eq!(peer_page.items.len(), 1);
    assert_eq!(peer_page.items[0].title, "PeerOnly");
}

uf_notifications_core::inventory::submit! {
    uf_notifications_core::NotificationKind {
        id: "apitest.approve",
        app: "apitest",
        label: "Request to approve",
        description: "Someone asked you to approve access.",
        defaults: uf_notifications_core::Channels { in_app: true, email: true, text: false },
        asks_action: true,
        urgent: true,
    }
}

uf_notifications_core::inventory::submit! {
    uf_notifications_core::NotificationKind {
        id: "apitest.answered",
        app: "apitest",
        label: "Request answered",
        description: "Your access request was answered.",
        defaults: uf_notifications_core::Channels::IN_APP_ONLY,
        asks_action: false,
        urgent: false,
    }
}

uf_notifications_core::inventory::submit! {
    uf_notifications_core::NotificationKind {
        id: "otherapp.mention",
        app: "otherapp",
        label: "Mentioned you",
        description: "Someone mentioned you.",
        defaults: uf_notifications_core::Channels { in_app: true, email: true, text: false },
        asks_action: true,
        urgent: false,
    }
}

async fn mint_kind(system: &valence::Valence, user: &str, kind: &str, title: &str) {
    send_notification(
        SendNotification {
            kind: kind.into(),
            ..sample(user, title)
        },
        system,
    )
    .await
    .expect("system mint");
}

async fn inbox_titles(
    v: &valence::Valence,
    user: &str,
    query: Option<&str>,
    filter: uf_notifications_api::NotificationInboxFilter,
) -> Vec<String> {
    let mut titles: Vec<String> = uf_notifications_api::inbox_page_for_request(
        v,
        RecordId::new("user", user),
        &PageRequest::new(0, 50),
        query.map(str::to_string),
        NotificationReadFilter::All,
        &filter,
    )
    .await
    .expect("inbox page")
    .items
    .into_iter()
    .map(|n| n.title)
    .collect();
    titles.sort();
    titles
}

#[tokio::test]
async fn inbox_filter_by_app_and_asks_action_happy_and_empty_sad() {
    use uf_notifications_api::NotificationInboxFilter;

    let base = setup_shared_db().await;
    let system = as_system(&base);
    let owner = as_user(&base, TEST_USER_A);
    mint_kind(&system, TEST_USER_A, "apitest.approve", "approve me").await;
    mint_kind(&system, TEST_USER_A, "apitest.answered", "answered").await;
    mint_kind(&system, TEST_USER_A, "otherapp.mention", "mention").await;
    mint_kind(&system, TEST_USER_A, "test", "unregistered").await;
    mint_kind(&system, TEST_USER_B, "apitest.approve", "peer approve").await;

    let all = inbox_titles(
        &owner,
        TEST_USER_A,
        None,
        NotificationInboxFilter::default(),
    )
    .await;
    assert_eq!(all, ["answered", "approve me", "mention", "unregistered"]);

    let by_app = NotificationInboxFilter {
        app_id: Some("apitest".into()),
        ..Default::default()
    };
    assert_eq!(
        inbox_titles(&owner, TEST_USER_A, None, by_app.clone()).await,
        ["answered", "approve me"]
    );

    let acting = NotificationInboxFilter {
        asks_action: true,
        ..Default::default()
    };
    assert_eq!(
        inbox_titles(&owner, TEST_USER_A, None, acting.clone()).await,
        ["approve me", "mention"]
    );

    let both = NotificationInboxFilter {
        asks_action: true,
        app_id: Some("apitest".into()),
    };
    assert_eq!(
        inbox_titles(&owner, TEST_USER_A, None, both).await,
        ["approve me"]
    );

    // Search combines with the filter.
    assert_eq!(
        inbox_titles(&owner, TEST_USER_A, Some("mention"), acting).await,
        ["mention"]
    );

    // A filter no registered kind matches is an empty page, not every row.
    let page = uf_notifications_api::inbox_page_for_request(
        &owner,
        RecordId::new("user", TEST_USER_A),
        &PageRequest::new(0, 50),
        None,
        NotificationReadFilter::All,
        &NotificationInboxFilter {
            app_id: Some("no-such-app".into()),
            ..Default::default()
        },
    )
    .await
    .expect("empty filter page");
    assert_eq!(page.items.len(), 0);
    assert_eq!(page.total_count, Some(0));

    // The DTO carries the registry fields.
    let rows = list_for_user(&owner, RecordId::new("user", TEST_USER_A))
        .await
        .unwrap();
    let approve = rows.iter().find(|n| n.title == "approve me").unwrap();
    assert_eq!(approve.kind_id.as_deref(), Some("apitest.approve"));
    assert_eq!(approve.kind_label.as_deref(), Some("Request to approve"));
    assert_eq!(approve.app_id.as_deref(), Some("apitest"));
    assert!(approve.asks_action);
    let unregistered = rows.iter().find(|n| n.title == "unregistered").unwrap();
    assert_eq!(unregistered.app_id, None);
}

#[tokio::test]
async fn preferences_for_user_lists_every_kind_and_follows_saved_choice() {
    use uf_notifications_core::{set_channel, Channel};

    let base = setup_shared_db().await;
    let owner = as_user(&base, TEST_USER_A);
    let me = RecordId::new("user", TEST_USER_A);

    let prefs = uf_notifications_api::preferences_for_user(&owner, &me)
        .await
        .expect("preferences");
    let ids: Vec<_> = prefs.kinds.iter().map(|k| k.kind.id.as_str()).collect();
    assert_eq!(
        ids,
        ["apitest.answered", "apitest.approve", "otherapp.mention"]
    );
    assert!(!prefs.email_verified && !prefs.text_verified);
    let approve = &prefs.kinds[1];
    assert!(
        approve.kind.defaults.email,
        "default is listed as registered"
    );
    assert!(!approve.channels.email, "email masked until verified");
    assert!(approve.channels.in_app);
    assert_eq!(
        prefs.settings.digest,
        uf_notifications_api::DigestFrequency::Instant
    );

    set_channel(&owner, &me, "apitest.approve", Channel::InApp, false)
        .await
        .unwrap();
    let after = uf_notifications_api::preferences_for_user(&owner, &me)
        .await
        .unwrap();
    assert!(!after.kinds[1].channels.in_app, "saved choice wins");
}
