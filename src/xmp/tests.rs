use super::parse::{CRS, RDF};
use super::*;
use crate::{develop::Recipe, raw::Metadata};
use anyhow::Result;
use std::path::Path;
fn xml(attrs: &str, body: &str) -> String {
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><r:RDF xmlns:r="{RDF}"><r:Description xmlns:c="{CRS}" {attrs}>{body}</r:Description></r:RDF></x:xmpmeta>"#
    )
}
#[test]
fn legacy_split_toning_restores_full_overlap_but_modern_presets_preserve_it() -> Result<()> {
    let mut base = Recipe::default();
    base.grading[1] = [0.3, 0.4, 0.2];
    base.effects.global_grade = [0.5, 0.6, -0.1];
    base.effects.blending = 0.2;
    let old = parse(
        Path::new("old.xmp"),
        &xml(
            r#"c:SplitToningShadowHue="240" c:SplitToningShadowSaturation="50""#,
            "",
        ),
    )?;
    let r = old.apply(&base, &Metadata::default(), &[], None)?;
    assert!(r.reference_color);
    assert_eq!(r.grading[0], [2. / 3., 0.5, 0.]);
    assert_eq!(r.effects.blending, 1.);
    assert_eq!(r.grading[1], [0.; 3]);
    assert_eq!(r.effects.global_grade, [0.; 3]);
    let modern = parse(
        Path::new("modern.xmp"),
        &xml(
            r#"c:SplitToningShadowHue="240" c:ColorGradeBlending="75""#,
            "",
        ),
    )?;
    let r = modern.apply(&base, &Metadata::default(), &[], None)?;
    assert_eq!(r.effects.blending, 0.75);
    assert_eq!(r.grading[1], base.grading[1]);
    assert_eq!(r.effects.global_grade, base.effects.global_grade);
    Ok(())
}

#[test]
fn photo_sidecar_empty_point_colors_and_preset_provenance() {
    let sentinel = vec!["-1.000000"; 19].join(", ");
    let body = format!(
        "<c:PointColors><r:Seq><r:li>{sentinel}</r:li></r:Seq></c:PointColors><c:ColorVariance><r:Seq><r:li>-50</r:li></r:Seq></c:ColorVariance><c:Preset><r:Description c:Amount=\"100\"><c:Parameters><r:Description c:Exposure2012=\"7\"/></c:Parameters></r:Description></c:Preset>"
    );
    let attrs = "xmlns:ps=\"http://ns.adobe.com/photoshop/1.0/\" ps:SidecarForExtension=\"RAF\" c:Exposure2012=\"0.5\" c:HDREditMode=\"0\" c:CurveRefineSaturation=\"100\"";
    let text = xml(attrs, &body);
    let p = parse(Path::new("photo.xmp"), &text).unwrap();
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    let r = p
        .apply(&Recipe::default(), &Metadata::default(), &[], None)
        .unwrap();
    assert_eq!(r.exposure, 0.5);
    let active = text.replacen("-1.000000", "0.25", 1);
    assert!(
        !parse(Path::new("active.xmp"), &active)
            .unwrap()
            .blockers
            .is_empty()
    );
    let hdr = text.replace("c:HDREditMode=\"0\"", "c:HDREditMode=\"1\"");
    assert!(
        parse(Path::new("hdr.xmp"), &hdr)
            .unwrap()
            .apply(&Recipe::default(), &Metadata::default(), &[], None)
            .is_err()
    );
}

#[test]
fn resolved_photo_white_balance_differs_from_as_shot_preset() -> Result<()> {
    let m = Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let attrs = r#"c:WhiteBalance="As Shot" c:Temperature="6500" c:Tint="25""#;
    let preset = parse(Path::new("preset.xmp"), &xml(attrs, ""))?;
    let result = preset.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!(result.wb, [1.; 3]);
    let sidecar = parse(
        Path::new("photo.xmp"),
        &xml(
            &format!(
                r#"{attrs} xmlns:ps="http://ns.adobe.com/photoshop/1.0/" ps:SidecarForExtension="RAF""#
            ),
            "",
        ),
    )?;
    let result = sidecar.apply(&Recipe::default(), &m, &[], None)?;
    assert_eq!(result.temperature, 6500.);
    assert_eq!(result.tint, 25.);
    assert_ne!(result.wb, [1.; 3]);
    Ok(())
}
#[test]
fn partial_preset_preserves_omitted_settings_and_zero_resets() -> Result<()> {
    let p = parse(
        Path::new("toolkit.xmp"),
        &xml(
            r#"c:Contrast2012="0" c:Saturation="-20" c:ConvertToGrayscale="False""#,
            "",
        ),
    )?;
    let mut r = Recipe {
        exposure: 1.7,
        contrast: 0.5,
        temperature: 4700.,
        ..Default::default()
    };
    r.effects.monochrome = true;
    let out = p.apply(&r, &Metadata::default(), &[], None)?;
    assert_eq!(out.exposure, 1.7);
    assert_eq!(out.temperature, 4700.);
    assert_eq!(out.contrast, 0.);
    assert!((out.saturation + 0.2).abs() < 1e-6);
    assert!(!out.effects.monochrome);
    Ok(())
}
#[test]
fn names_entities_and_rgb_curves_parse_with_namespace_aliases() -> Result<()> {
    let p = parse(
        Path::new("preset.xmp"),
        &xml(
            "",
            r#"<c:Name><r:Alt><r:li xml:lang="x-default">Warm &amp; soft</r:li></r:Alt></c:Name><c:ToneCurvePV2012Red><r:Seq><r:li>1, 12</r:li><r:li>240, 250</r:li></r:Seq></c:ToneCurvePV2012Red>"#,
        ),
    )?;
    assert_eq!(p.name, "Warm & soft");
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.curve, ToneCurve::default());
    assert_eq!(r.effects.channels[0].points.len(), 2);
    assert_eq!(r.effects.channels[0].points[0], [1. / 255., 12. / 255.]);
    assert_eq!(r.effects.channels[0].evaluate(0.), 12. / 255.);
    assert_eq!(r.effects.channels[0].evaluate(1.), 250. / 255.);
    assert!(r.reference_curves);
    Ok(())
}
#[test]
fn child_scalar_values_and_multiple_descriptions() -> Result<()> {
    let text = format!(
        r#"<r:RDF xmlns:r="{RDF}" xmlns:c="{CRS}" xmlns:d="urn:other"><r:Description><d:Exposure2012>99</d:Exposure2012></r:Description><r:Description><c:Exposure2012>+0.5</c:Exposure2012><c:Clarity2012>-20</c:Clarity2012></r:Description></r:RDF>"#
    );
    let p = parse(Path::new("p.xmp"), &text)?;
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.5);
    assert!((r.effects.clarity + 0.2).abs() < 1e-6);
    Ok(())
}
#[test]
fn missing_profiles_unknown_settings_and_looks_never_partially_apply() -> Result<()> {
    let base = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    for attrs in [
        r#"c:Exposure2012="2" c:CameraProfile="Missing""#,
        r#"c:Exposure2012="2" c:ImaginaryControl="0""#,
    ] {
        let p = parse(Path::new("p.xmp"), &xml(attrs, ""))?;
        assert!(p.apply(&base, &Metadata::default(), &[], None).is_err());
        assert_eq!(base.exposure, 1.);
    }
    let p = parse(
        Path::new("look.xmp"),
        &xml(
            r#"c:Exposure2012="2""#,
            r#"<c:Look><r:Description c:Name="Adobe Color" c:Exposure2012="7"/></c:Look>"#,
        ),
    )?;
    assert_eq!(p.settings["Exposure2012"], "2");
    assert!(p.apply(&base, &Metadata::default(), &[], None).is_err());
    Ok(())
}
#[test]
fn malformed_numbers_rejected() -> Result<()> {
    for value in ["NaN", "inf", "oops", "900"] {
        let p = parse(
            Path::new("p.xmp"),
            &xml(&format!("c:Exposure2012=\"{value}\""), ""),
        )?;
        assert!(
            p.apply(&Recipe::default(), &Metadata::default(), &[], None)
                .is_err()
        );
    }
    Ok(())
}
#[test]
fn specific_duplicate_group_repair_is_reported() -> Result<()> {
    let text = format!(
        r#"<rdf:RDF xmlns:rdf="{RDF}" xmlns:crs="{CRS}"><rdf:Description crs:Contrast2012="1">
<crs:Group><rdf:Alt><rdf:li>Film</rdf:li></rdf:Alt>
</crs:Group>
</crs:Group>
</rdf:Description></rdf:RDF>"#
    );
    let p = parse(Path::new("p.xmp"), &text)?;
    assert_eq!(p.group, "Film");
    assert_eq!(p.notes.len(), 1);
    Ok(())
}
#[test]
fn lenient_apply_keeps_supported_settings_and_reports_the_rest() -> Result<()> {
    let base = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let p = parse(
        Path::new("p.xmp"),
        &xml(r#"c:Exposure2012="2" c:CameraProfile="Missing""#, ""),
    )?;
    let (recipe, skipped) = p.apply_lenient(&base, &Metadata::default(), &[], None)?;
    assert_eq!(recipe.exposure, 2.);
    assert_eq!(recipe.profile, base.profile);
    assert!(skipped.iter().any(|s| s.contains("Missing camera profile")));
    Ok(())
}
#[test]
fn empty_flags_are_unset_and_curves_still_apply() -> Result<()> {
    let p = parse(
        Path::new("p.xmp"),
        &xml(
            r#"c:ConvertToGrayscale="" c:ToneCurveName2012="Custom""#,
            r#"<c:ToneCurvePV2012><r:Seq><r:li>0, 50</r:li><r:li>255, 255</r:li></r:Seq></c:ToneCurvePV2012>"#,
        ),
    )?;
    let r = p.apply(&Recipe::default(), &Metadata::default(), &[], None)?;
    assert!(!r.effects.monochrome);
    assert_eq!(r.curve.points[0], [0., 50. / 255.]);
    Ok(())
}
