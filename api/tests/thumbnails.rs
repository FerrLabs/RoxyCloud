mod common;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use http_body_util::BodyExt;
use roxycloud_api::build_router;
use roxycloud_core::role::Role;
use tower::ServiceExt;
use uuid::Uuid;

use common::Harness;

struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Answer {
    fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
}

async fn get(harness: &Harness, bearer: &str, path: &str) -> Answer {
    let response = build_router(harness.state.clone(), &[], None)
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
                .body(Body::empty())
                .expect("a well formed request"),
        )
        .await
        .expect("the router answers");

    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("reading the body")
        .to_bytes()
        .to_vec();

    Answer {
        status,
        headers,
        body,
    }
}

async fn session(harness: &Harness, email: &str) -> (Uuid, String) {
    let user = harness.account(email, Role::Member).await;
    let token = harness.state.sessions.issue(user.id).expect("a token");
    (user.id, token)
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([
            u8::try_from(x % 256).unwrap_or(0),
            u8::try_from(y % 256).unwrap_or(0),
            200,
        ])
    });
    let mut encoded = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut encoded, image::ImageFormat::Png)
        .expect("encoding the fixture");
    encoded.into_inner()
}

database_test!(a_thumbnail_is_made_on_demand_and_kept, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "holiday.png", &png(900, 600)).await;

    let first = get(&harness, &bearer, "/v1/thumbnails/holiday.png?edge=256").await;
    let cached = harness.thumbnail_rows().await;
    let second = get(&harness, &bearer, "/v1/thumbnails/holiday.png?edge=256").await;

    assert_eq!(first.status, StatusCode::OK);
    let decoded = image::load_from_memory(&first.body).expect("a real image comes back");
    assert!(decoded.width() <= 256 && decoded.height() <= 256);
    assert_eq!(cached, 1, "the second request should not decode again");
    assert_eq!(second.body, first.body);
});

database_test!(
    the_same_bytes_under_two_names_share_one_thumbnail,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com").await;
        let bytes = png(400, 300);
        harness.write(id, "one.png", &bytes).await;
        harness.write(id, "two.png", &bytes).await;

        get(&harness, &bearer, "/v1/thumbnails/one.png?edge=128").await;
        get(&harness, &bearer, "/v1/thumbnails/two.png?edge=128").await;

        assert_eq!(
            harness.thumbnail_rows().await,
            1,
            "the cache is keyed on the source digest, so identical files cost one thumbnail"
        );
    }
);

database_test!(a_thumbnail_is_not_served_as_something_to_render, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "holiday.png", &png(400, 300)).await;

    let answer = get(&harness, &bearer, "/v1/thumbnails/holiday.png").await;

    assert_eq!(answer.header("content-type"), "application/octet-stream");
    assert_eq!(answer.header("content-disposition"), "attachment");
    assert_eq!(
        answer.header("x-content-type-options"),
        "nosniff",
        "one exception is how the rule that nothing renders on this origin stops being true"
    );
});

database_test!(what_is_not_an_image_gets_no_thumbnail, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "report.pdf", b"not an image").await;
    harness
        .write(
            id,
            "diagram.svg",
            b"<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>",
        )
        .await;

    let pdf = get(&harness, &bearer, "/v1/thumbnails/report.pdf").await;
    let svg = get(&harness, &bearer, "/v1/thumbnails/diagram.svg").await;

    assert_eq!(pdf.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        svg.status,
        StatusCode::BAD_REQUEST,
        "an SVG is a document with a script surface, not a raster to shrink"
    );
    assert_eq!(harness.thumbnail_rows().await, 0);
});

database_test!(the_name_decides_before_the_bytes_are_read, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    // Real PNG bytes under a name that does not claim to be an image. The decoder would take
    // these happily, so what refuses them is the name, before the blob is read into memory at all.
    harness.write(id, "invoice.pdf", &png(400, 300)).await;

    let answer = get(&harness, &bearer, "/v1/thumbnails/invoice.pdf").await;

    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(harness.thumbnail_rows().await, 0);
});

database_test!(a_size_nobody_offered_is_refused, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    harness.write(id, "holiday.png", &png(400, 300)).await;

    let absurd = get(&harness, &bearer, "/v1/thumbnails/holiday.png?edge=100000").await;
    let odd = get(&harness, &bearer, "/v1/thumbnails/holiday.png?edge=257").await;

    assert_eq!(
        absurd.status,
        StatusCode::BAD_REQUEST,
        "an open size lets one request make the server do any amount of work"
    );
    assert_eq!(odd.status, StatusCode::BAD_REQUEST);
});

database_test!(
    a_thumbnail_is_never_of_a_file_the_caller_does_not_own,
    harness,
    {
        let (mine, bearer) = session(&harness, "mine@example.com").await;
        let (theirs, _) = session(&harness, "theirs@example.com").await;
        harness.write(theirs, "private.png", &png(400, 300)).await;
        harness.root(mine).await;

        let answer = get(&harness, &bearer, "/v1/thumbnails/private.png").await;

        assert_eq!(answer.status, StatusCode::NOT_FOUND);
    }
);

database_test!(
    the_bytes_a_thumbnail_points_at_survive_the_sweep,
    harness,
    {
        let (id, bearer) = session(&harness, "owner@example.com").await;
        harness.write(id, "holiday.png", &png(400, 300)).await;
        get(&harness, &bearer, "/v1/thumbnails/holiday.png").await;

        let thumbnail = harness.thumbnail_blob().await;

        assert_eq!(
            harness.blob_references(thumbnail).await,
            1,
            "no node points at a thumbnail, so without a reference of its own the sweep takes it"
        );
    }
);

database_test!(a_deleted_photo_does_not_keep_its_thumbnails, harness, {
    let (id, bearer) = session(&harness, "owner@example.com").await;
    let node = harness.write(id, "holiday.png", &png(400, 300)).await;
    get(&harness, &bearer, "/v1/thumbnails/holiday.png").await;
    let thumbnail = harness.thumbnail_blob().await;

    harness.trash(&node).await;
    harness.purge(id, node.id).await;
    let dropped = roxycloud_api::thumbnails::forget_orphans(&harness.state.db)
        .await
        .expect("forgetting");

    assert_eq!(dropped, 1);
    assert_eq!(harness.thumbnail_rows().await, 0);
    assert_eq!(
        harness.blob_references(thumbnail).await,
        0,
        "the reference has to go, or the bytes stay for good"
    );
});
