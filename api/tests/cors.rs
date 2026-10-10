mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use stashden_api::build_router;
use tower::ServiceExt;

use common::Harness;

const SPA: &str = "http://localhost:4200";

async fn preflight(
    harness: &Harness,
    uri: &str,
    method: &str,
    headers: &str,
) -> (StatusCode, String, String) {
    let response = build_router(harness.state.clone(), &[SPA.to_owned()], None)
        .oneshot(
            Request::builder()
                .method("OPTIONS")
                .uri(uri)
                .header(header::ORIGIN, SPA)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, headers)
                .body(Body::empty())
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers");
    let text = |name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    (
        response.status(),
        text(header::ACCESS_CONTROL_ALLOW_METHODS),
        text(header::ACCESS_CONTROL_ALLOW_HEADERS),
    )
}

database_test!(a_browser_may_send_a_chunk_to_a_resumable_upload, harness, {
    let (status, methods, headers) = preflight(
        &harness,
        "/v1/uploads/00000000-0000-0000-0000-000000000000",
        "PATCH",
        "authorization,upload-offset",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(methods.contains("patch"), "{methods}");
    assert!(headers.contains("upload-offset"), "{headers}");
});

database_test!(a_browser_may_name_the_version_it_is_replacing, harness, {
    let (status, _, headers) = preflight(
        &harness,
        "/v1/files/a.txt",
        "PUT",
        "authorization,if-match,if-none-match",
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert!(headers.contains("if-match"), "{headers}");
    assert!(headers.contains("if-none-match"), "{headers}");
});
