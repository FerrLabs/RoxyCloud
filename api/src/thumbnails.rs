use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};
use sqlx::{PgPool, Postgres, Transaction};

use crate::error::ApiError;
use roxycloud_core::blob::BlobHash;

/// The sizes a caller may ask for. An open integer would let one request decode and re-encode at
/// any dimension it likes, which is a cheap way to make the server do expensive work.
pub const EDGES: [i32; 3] = [128, 256, 512];

/// Nothing larger is decoded. A file past this is refused on its size alone, before a decoder sees
/// it, which is the cheapest bound available.
pub const LARGEST_SOURCE_BYTES: u64 = 32 * 1024 * 1024;

/// Nothing with more pixels than this is decoded, whatever it weighs. A few hundred kilobytes of
/// PNG can declare a canvas of hundreds of megapixels, and the allocation is what takes the server
/// down rather than the file.
pub const LARGEST_PIXELS: u64 = 50_000_000;

/// What the decoder may allocate for one image, which bounds the damage even for a file whose
/// header lies about its dimensions.
const LARGEST_ALLOCATION: u64 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum NotAnImage {
    #[error("no thumbnail is made of this kind of file")]
    Format,
    #[error("this image is too large to make a thumbnail of")]
    TooBig,
    #[error("this image could not be read")]
    Unreadable,
}

#[must_use]
pub fn is_an_edge(edge: i32) -> bool {
    EDGES.contains(&edge)
}

/// Only these are decoded. Everything else, `SVG` above all, is refused: an `SVG` is a document
/// with a script surface rather than a raster to shrink, and no thumbnail is worth opening that.
#[must_use]
pub fn looks_like_an_image(name: &str) -> bool {
    let extension = name
        .rsplit_once('.')
        .map(|(_, rest)| rest.to_ascii_lowercase());
    matches!(
        extension.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    )
}

/// Decodes, shrinks and re-encodes to `WebP`, inside every bound the module declares.
///
/// This runs in process. The decision is deliberate: the decoders are pure Rust, so the failure
/// mode that makes image handling notorious, a memory-safety bug in a C library reached by a
/// crafted file, is not on the table. What is left is resource exhaustion, and that is what the
/// caps above are for. A file is refused on its size before decoding, on its declared pixel count
/// before allocating, and the decoder is given an allocation ceiling in case the header lied.
pub fn shrink(source: &[u8], edge: u32) -> Result<Vec<u8>, NotAnImage> {
    if source.len() as u64 > LARGEST_SOURCE_BYTES {
        return Err(NotAnImage::TooBig);
    }

    let reader = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|_| NotAnImage::Unreadable)?;

    match reader.format() {
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP) => {}
        _ => return Err(NotAnImage::Format),
    }

    // Read out of the header, before anything is allocated. Checked after decoding it would be a
    // report rather than a bound: a sixty megapixel JPEG is a few compressed megabytes, passes the
    // size cap, and costs a hundred and eighty megabytes to find out about.
    let (width, height) = ImageReader::new(Cursor::new(source))
        .with_guessed_format()
        .map_err(|_| NotAnImage::Unreadable)?
        .into_dimensions()
        .map_err(|_| NotAnImage::Unreadable)?;
    if u64::from(width) * u64::from(height) > LARGEST_PIXELS {
        return Err(NotAnImage::TooBig);
    }

    let mut reader = reader;
    let mut limits = Limits::default();
    limits.max_alloc = Some(LARGEST_ALLOCATION);
    reader.limits(limits);

    let decoded = reader.decode().map_err(|_| NotAnImage::Unreadable)?;

    // `thumbnail` scales up as readily as down, and blowing a small image up costs work and
    // quality to produce something worse than the file already was.
    let thumbnail = if decoded.width() <= edge && decoded.height() <= edge {
        decoded
    } else {
        decoded.thumbnail(edge, edge)
    };
    let mut encoded = Cursor::new(Vec::new());
    thumbnail
        .write_to(&mut encoded, ImageFormat::WebP)
        .map_err(|_| NotAnImage::Unreadable)?;

    Ok(encoded.into_inner())
}

pub async fn cached(
    pool: &PgPool,
    source: BlobHash,
    edge: i32,
) -> Result<Option<BlobHash>, ApiError> {
    sqlx::query_scalar::<_, BlobHash>(
        "SELECT blob_hash FROM thumbnails WHERE source_hash = $1 AND edge = $2",
    )
    .bind(source)
    .bind(edge)
    .fetch_optional(pool)
    .await
    .map_err(Into::into)
}

/// Remembers a thumbnail and takes a reference to its bytes, so the sweep does not collect
/// something no node points at but the cache does.
pub async fn remember(
    tx: &mut Transaction<'_, Postgres>,
    source: BlobHash,
    edge: i32,
    thumbnail: BlobHash,
) -> Result<(), ApiError> {
    let inserted = sqlx::query(
        "INSERT INTO thumbnails (source_hash, edge, blob_hash)
         VALUES ($1, $2, $3)
         ON CONFLICT (source_hash, edge) DO NOTHING",
    )
    .bind(source)
    .bind(edge)
    .bind(thumbnail)
    .execute(&mut **tx)
    .await?;

    if inserted.rows_affected() > 0 {
        crate::db::acquire_blob_for_cache(tx, thumbnail).await?;
    }
    Ok(())
}

/// Drops the thumbnails of a source nothing points at any more, releasing their bytes so the sweep
/// can take them on a later pass. Without this a deleted photo keeps its thumbnails for good.
pub async fn forget_orphans(pool: &PgPool) -> Result<u64, ApiError> {
    // One transaction for the whole batch. Deleting the rows on their own and releasing after
    // would, on a failure partway, leave rows gone and their references held by nothing that can
    // ever release them, which is this function's own leak from the other side.
    let mut tx = pool.begin().await?;
    let orphaned = sqlx::query_scalar::<_, BlobHash>(
        "DELETE FROM thumbnails
         WHERE NOT EXISTS (
             SELECT 1 FROM nodes
             WHERE nodes.blob_hash = thumbnails.source_hash AND nodes.deleted_at IS NULL
         )
         RETURNING blob_hash",
    )
    .fetch_all(&mut *tx)
    .await?;

    let mut released = 0;
    for hash in orphaned {
        crate::db::release_blob(&mut tx, hash).await?;
        released += 1;
    }
    tx.commit().await?;
    Ok(released)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([
                u8::try_from(x % 256).unwrap_or(0),
                u8::try_from(y % 256).unwrap_or(0),
                128,
            ])
        });
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encoding the fixture");
        encoded.into_inner()
    }

    #[test]
    fn a_thumbnail_fits_inside_the_edge_it_was_asked_for() {
        let shrunk = shrink(&png(800, 400), 256).expect("shrinks");
        let decoded = image::load_from_memory(&shrunk).expect("decodes");

        assert!(decoded.width() <= 256 && decoded.height() <= 256);
        assert!(
            decoded.width() > decoded.height(),
            "a wide image should stay wide"
        );
    }

    #[test]
    fn an_image_smaller_than_the_edge_is_not_blown_up() {
        let shrunk = shrink(&png(60, 40), 256).expect("shrinks");
        let decoded = image::load_from_memory(&shrunk).expect("decodes");

        assert_eq!((decoded.width(), decoded.height()), (60, 40));
    }

    #[test]
    fn what_is_not_an_image_is_refused_rather_than_decoded() {
        assert!(matches!(
            shrink(
                b"<svg xmlns='http://www.w3.org/2000/svg'><script/></svg>",
                256
            ),
            Err(NotAnImage::Format | NotAnImage::Unreadable)
        ));
        assert!(matches!(
            shrink(b"not an image at all", 256),
            Err(NotAnImage::Format | NotAnImage::Unreadable)
        ));
    }

    #[test]
    fn a_file_past_the_size_cap_is_refused_before_anything_decodes_it() {
        let enormous = vec![0u8; usize::try_from(LARGEST_SOURCE_BYTES).expect("fits") + 1];
        assert!(matches!(shrink(&enormous, 256), Err(NotAnImage::TooBig)));
    }

    #[test]
    fn a_lying_header_is_caught_before_anything_is_allocated() {
        // A canvas past the cap in a file well under the size cap: what the pixel bound exists for.
        let mut declared = png(1, 1);
        // The IHDR width and height sit at a fixed offset in a PNG, so a real decoder reads these
        // before allocating and so must the bound.
        declared[16..20].copy_from_slice(&30_000u32.to_be_bytes());
        declared[20..24].copy_from_slice(&30_000u32.to_be_bytes());

        assert!(
            (u64::from(30_000u32) * u64::from(30_000u32)) > LARGEST_PIXELS,
            "the fixture has to actually exceed the cap"
        );
        assert!(matches!(
            shrink(&declared, 256),
            Err(NotAnImage::TooBig | NotAnImage::Unreadable)
        ));
    }

    #[test]
    fn only_the_offered_edges_are_accepted() {
        assert!(is_an_edge(256));
        assert!(!is_an_edge(257));
        assert!(!is_an_edge(0));
        assert!(!is_an_edge(100_000));
    }

    #[test]
    fn a_name_decides_whether_the_bytes_are_worth_opening() {
        assert!(looks_like_an_image("holiday.JPG"));
        assert!(looks_like_an_image("chart.png"));
        assert!(!looks_like_an_image("diagram.svg"));
        assert!(!looks_like_an_image("report.pdf"));
        assert!(!looks_like_an_image("noextension"));
    }
}
