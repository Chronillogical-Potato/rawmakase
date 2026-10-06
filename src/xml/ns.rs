//! XMP namespace URIs, and the headers that mark XMP in a JPEG segment.

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
/// Of `xml:lang`.
pub const XML: &str = "http://www.w3.org/XML/1998/namespace";
/// Camera Raw settings (`crs:`).
pub const CRS: &str = "http://ns.adobe.com/camera-raw-settings/1.0/";
pub const XMP: &str = "http://ns.adobe.com/xap/1.0/";
pub const XMP_MM: &str = "http://ns.adobe.com/xap/1.0/mm/";
pub const XMP_NOTE: &str = "http://ns.adobe.com/xmp/note/";
pub const DC: &str = "http://purl.org/dc/elements/1.1/";
pub const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
pub const EXIF: &str = "http://ns.adobe.com/exif/1.0/";
pub const AUX: &str = "http://ns.adobe.com/exif/1.0/aux/";
pub const LR: &str = "http://ns.adobe.com/lightroom/1.0/";
pub const DIGIKAM: &str = "http://www.digikam.org/ns/1.0/";
/// Of an Adobe lens profile's camera models (`stCamera:`).
pub const ST_CAMERA: &str = "http://ns.adobe.com/photoshop/1.0/camera-profile";

/// What starts a JPEG APP1 segment holding the standard XMP packet.
pub const JPEG_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
/// What starts each APP1 segment of ExtendedXMP.
pub const JPEG_EXTENDED_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
