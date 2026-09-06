mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

struct Answer {
    status: StatusCode,
    body: String,
}

impl Answer {
    /// The names the search came back with, in the order it put them.
    fn names(&self) -> Vec<String> {
        self.body
            .split("\"name\":\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(ToOwned::to_owned)
            .collect()
    }

    fn paths(&self) -> Vec<String> {
        self.body
            .split("\"path\":\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(ToOwned::to_owned)
            .collect()
    }
}

async fn search(harness: &Harness, bearer: &str, query: &str) -> Answer {
    let encoded = query
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('&', "%26")
        .replace('+', "%2B")
        .replace('#', "%23");
    request(harness, bearer, &format!("/v1/search?q={encoded}")).await
}

async fn request(harness: &Harness, bearer: &str, uri: &str) -> Answer {
    let mut builder = Request::builder().method("GET").uri(uri);
    if !bearer.is_empty() {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {bearer}"));
    }

    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(builder.body(Body::empty()).expect("a well formed request"))
        .await
        .expect("the router answers");

    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes();

    Answer {
        status,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

async fn session(harness: &Harness, email: &str) -> (Uuid, String) {
    let user = harness.account(email, Role::Member).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, token)
}

database_test!(a_search_finds_a_file_by_part_of_its_name, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "quarterly-report.pdf", b"figures").await;
    harness.write(id, "holiday-photo.jpg", b"beach").await;

    let found = search(&harness, &bearer, "report").await;

    assert_eq!(found.status, StatusCode::OK, "{}", found.body);
    assert_eq!(found.names(), ["quarterly-report.pdf"]);
});

database_test!(
    results_never_leak_a_node_the_caller_does_not_own,
    harness,
    {
        let (mine, bearer) = session(&harness, "mine@example.com").await;
        let (theirs, _) = session(&harness, "theirs@example.com").await;
        harness.write(mine, "shared-name.txt", b"mine").await;
        harness.write(theirs, "shared-name.txt", b"theirs").await;
        harness.write(theirs, "another-name.txt", b"theirs").await;

        let found = search(&harness, &bearer, "name").await;

        assert_eq!(found.names(), ["shared-name.txt"]);
        assert!(
            !found.body.contains(&theirs.to_string()),
            "a search that names another account's node has leaked it whatever else it did: {}",
            found.body
        );
    }
);

database_test!(
    a_path_never_names_a_directory_in_another_account,
    harness,
    {
        let (mine, bearer) = session(&harness, "mine@example.com").await;
        let (theirs, _) = session(&harness, "theirs@example.com").await;
        harness
            .write(theirs, "confidential-plans/decoy.txt", b"theirs")
            .await;
        let foreign = harness.resolve(theirs, "confidential-plans").await;
        let ours = harness.write(mine, "budget.md", b"mine").await;

        harness.graft(ours.id, foreign.id).await;
        let found = search(&harness, &bearer, "budget").await;

        assert_eq!(found.names(), ["budget.md"]);
        assert_eq!(
            found.paths(),
            ["budget.md"],
            "the walk up has to stop at the account boundary rather than trusting the tree to be intact"
        );
    }
);

database_test!(the_path_says_where_the_match_is, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness
        .write(id, "work/2026/q1/summary.md", b"figures")
        .await;
    harness.write(id, "summary.md", b"at the top").await;

    let found = search(&harness, &bearer, "summary").await;

    let mut paths = found.paths();
    paths.sort();
    assert_eq!(paths, ["summary.md", "work/2026/q1/summary.md"]);
});

database_test!(a_directory_is_found_the_same_way_a_file_is, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "invoices/2026/march.pdf", b"paid").await;

    let found = search(&harness, &bearer, "invoices").await;

    assert_eq!(found.names(), ["invoices"]);
    assert_eq!(found.paths(), ["invoices"]);
});

database_test!(the_trash_is_not_searched, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    let node = harness.write(id, "report.pdf", b"figures").await;

    harness.trash(&node).await;
    let while_deleted = search(&harness, &bearer, "report").await;

    harness.restore(id, node.id).await;
    let once_restored = search(&harness, &bearer, "report").await;

    assert!(while_deleted.names().is_empty(), "{}", while_deleted.body);
    assert_eq!(once_restored.names(), ["report.pdf"]);
});

database_test!(a_name_that_starts_with_the_query_comes_first, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "annual-report.pdf", b"figures").await;
    harness.write(id, "report-draft.md", b"words").await;

    let found = search(&harness, &bearer, "report").await;

    assert_eq!(
        found.names(),
        ["report-draft.md", "annual-report.pdf"],
        "somebody who typed the beginning of a name is looking for that name"
    );
});

database_test!(what_somebody_typed_is_a_literal_not_a_pattern, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "a_b.txt", b"underscore").await;
    harness.write(id, "axb.txt", b"letter").await;
    harness.write(id, "100%.txt", b"percent").await;

    let underscore = search(&harness, &bearer, "a_b").await;
    let percent = search(&harness, &bearer, "%").await;

    assert_eq!(
        underscore.names(),
        ["a_b.txt"],
        "an underscore is one of anything to ILIKE, so axb.txt would match too"
    );
    assert_eq!(
        percent.names(),
        ["100%.txt"],
        "a percent sign is everything to ILIKE, so this would return the whole tree"
    );
});

database_test!(the_search_is_blind_to_case, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "Quarterly-Report.PDF", b"figures").await;

    let found = search(&harness, &bearer, "quarterly").await;

    assert_eq!(found.names(), ["Quarterly-Report.PDF"]);
});

database_test!(a_page_can_be_asked_for_by_itself, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    for name in ["note-a.md", "note-b.md", "note-c.md"] {
        harness.write(id, name, b"words").await;
    }

    let first = request(&harness, &bearer, "/v1/search?q=note&limit=2").await;
    let second = request(&harness, &bearer, "/v1/search?q=note&limit=2&offset=2").await;

    assert_eq!(first.names(), ["note-a.md", "note-b.md"]);
    assert_eq!(second.names(), ["note-c.md"]);
});

database_test!(a_search_for_nothing_is_refused, harness, {
    let (_, bearer) = session(&harness, "owner@example.com").await;

    let empty = request(&harness, &bearer, "/v1/search?q=").await;
    let blank = request(&harness, &bearer, "/v1/search?q=%20%20").await;
    let missing = request(&harness, &bearer, "/v1/search").await;

    assert_eq!(empty.status, StatusCode::BAD_REQUEST);
    assert_eq!(blank.status, StatusCode::BAD_REQUEST);
    assert_eq!(missing.status, StatusCode::BAD_REQUEST);
});

database_test!(searching_needs_a_session, harness, {
    let (id, _) = session(&harness, "owner@example.com").await;
    harness.write(id, "report.pdf", b"figures").await;

    let found = request(&harness, "", "/v1/search?q=report").await;

    assert_eq!(found.status, StatusCode::UNAUTHORIZED);
});

database_test!(
    a_nonsense_page_size_is_clamped_rather_than_obeyed,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com").await;
        for name in ["note-a.md", "note-b.md"] {
            harness.write(id, name, b"words").await;
        }

        let none = request(&harness, &bearer, "/v1/search?q=note&limit=0").await;
        let backwards = request(&harness, &bearer, "/v1/search?q=note&offset=-5").await;

        assert_eq!(
            none.names(),
            ["note-a.md"],
            "a page of nothing is a request nobody meant to make"
        );
        assert_eq!(
            backwards.status,
            StatusCode::OK,
            "a negative offset is an error to Postgres, so it has to be caught before it gets there: {}",
            backwards.body
        );
        assert_eq!(backwards.names(), ["note-a.md", "note-b.md"]);
    }
);
