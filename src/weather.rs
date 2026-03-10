use reqwest::Client;
use serde::Deserialize;

#[derive(Deserialize, Debug)]
pub struct WeatherResponse {
    pub main: Main,
    pub weather: Vec<Weather>,
}

#[derive(Deserialize, Debug)]
pub struct Main {
    pub temp: f64,
}

#[derive(Deserialize, Debug)]
pub struct Weather {
    pub description: String,
}

pub async fn get_weather(client: &Client, lat: f64, lng: f64, lang: &str) -> Result<String, String> {
    let api_key = std::env::var("OPENWEATHER_API_KEY").map_err(|_| "Missing OPENWEATHER_API_KEY".to_string())?;
    
    // Normalize lang for OpenWeather
    let ow_lang = match lang {
        "id" => "id",
        "en" => "en",
        "ja" => "ja",
        "ko" => "kr",
        "zh" => "zh_cn",
        "ru" => "ru",
        "nl" => "nl",
        "af" => "af",
        "ar" => "ar",
        _ => "en",
    };

    let url = format!(
        "https://api.openweathermap.org/data/2.5/weather?lat={}&lon={}&appid={}&units=metric&lang={}",
        lat, lng, api_key.replace("\"", ""), ow_lang
    );

    let res = client.get(&url).send().await.map_err(|e| e.to_string())?;
    
    if !res.status().is_success() {
        return Err(format!("OpenWeather error: {}", res.status()));
    }

    let weather_data: WeatherResponse = res.json().await.map_err(|e| e.to_string())?;
    
    let temp = weather_data.main.temp.round();
    let desc = weather_data.weather.first().map(|w| w.description.as_str()).unwrap_or("Unknown");
    
    // Capitalize first letter of description
    let mut chars = desc.chars();
    let capitalized_desc = match chars.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
    };

    Ok(format!("{}°C, {}", temp, capitalized_desc))
}
