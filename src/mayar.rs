use reqwest::Client;
use serde_json::Value;

pub async fn create_invoice(
    client: &Client,
    customer_name: &str,
    customer_email: &str,
    amount: f64,
    reference: &str,
    redirect_url: &str,
) -> Result<String, String> {
    
    let api_key = std::env::var("MAYAR_API_KEY").map_err(|_| "Missing MAYAR_API_KEY")?;
    
    // Sandbox URL for Mayar Invoice
    let base_url = std::env::var("MAYAR_BASE_URL").unwrap_or_else(|_| "https://api.mayar.club/hl/v1/invoice/create".to_string());

    let req_body = serde_json::json!({
        "name": customer_name,
        "email": customer_email,
        "mobile": "08123456789",
        "amount": amount,
        "description": format!("Purchase {}", reference),
        "redirect_url": redirect_url,
        "items": [
            {
                "name": format!("Package {}", reference),
                "description": "Ulyn Sosmed Plan",
                "quantity": 1,
                "rate": amount
            }
        ],
        "extra_data": {
            "reference": reference
        }
    });

    let response = client
        .post(base_url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&req_body)
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    let status = response.status();
    let resp_json: Value = response.json().await.map_err(|e| format!("Failed to parse response: {}", e))?;

    if status.is_success() {
        let payment_url = resp_json.get("data")
            .and_then(|data| data.get("link"))
            .and_then(|link| link.as_str())
            .map(|s| s.to_string())
            .unwrap_or_default();

        if !payment_url.is_empty() {
            Ok(payment_url)
        } else {
            Err(format!("Mayar Response data: {:?}", resp_json))
        }
    } else {
        Err(format!("Mayar API Error ({}): {:?}", status, resp_json))
    }
}
