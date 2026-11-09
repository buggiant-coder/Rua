use base64::{engine::general_purpose::STANDARD, Engine};
use image::Luma;
use qrcode::QrCode;

use crate::errors::ApiError;

pub fn generate_qrcode_image(scan_url: &str) -> Result<String, ApiError> {
    let code = QrCode::with_error_correction_level(scan_url, qrcode::EcLevel::M)
        .map_err(|e| ApiError::Internal(format!("QR code generation failed: {e}")))?;

    let image = code
        .render::<Luma<u8>>()
        .quiet_zone(true)
        .module_dimensions(6, 6)
        .build();

    let mut buf = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageLuma8(image)
        .write_to(&mut buf, image::ImageFormat::Png)
        .map_err(|e| ApiError::Internal(format!("PNG encoding failed: {e}")))?;

    Ok(STANDARD.encode(buf.into_inner()))
}
