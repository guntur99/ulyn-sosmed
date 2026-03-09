use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::Region;
use std::env;

pub async fn upload_to_spaces(path: &str, bytes: Vec<u8>, content_type: &str) -> Result<String, String> {
    let key = env::var("DO_SPACES_KEY").map_err(|_| "DO_SPACES_KEY not set")?;
    let secret = env::var("DO_SPACES_SECRET").map_err(|_| "DO_SPACES_SECRET not set")?;
    let endpoint = env::var("DO_SPACES_ENDPOINT").map_err(|_| "DO_SPACES_ENDPOINT not set")?;
    let region_str = env::var("DO_SPACES_REGION").unwrap_or_else(|_| "sgp1".to_string());
    let bucket_name = env::var("DO_SPACES_BUCKET").map_err(|_| "DO_SPACES_BUCKET not set")?;

    let credentials = Credentials::new(Some(&key), Some(&secret), None, None, None).map_err(|e| e.to_string())?;
    let region = Region::Custom {
        region: region_str,
        endpoint,
    };

    let mut bucket = Bucket::new(&bucket_name, region, credentials).map_err(|e| e.to_string())?;
    
    // Explicit headers (Standard S3 headers)
    bucket.add_header("x-amz-acl", "public-read");
    bucket.add_header("Cache-Control", "max-age=31536000, public");

    tracing::info!("Uploading to Spaces: {} ({} bytes, {})", path, bytes.len(), content_type);

    let response = bucket.put_object_with_content_type(path, &bytes, content_type)
        .await
        .map_err(|e| format!("S3 Upload Error: {}", e))?;
    
    tracing::info!("Upload finished with status: {}", response.status_code());

    Ok(path.to_string())
}
