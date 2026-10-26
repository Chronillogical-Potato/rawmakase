//! `rawmakase import-profiles` with `.cube` files and folders of them, against a
//! data folder of its own.
use anyhow::Result;
use std::{fs, path::Path, process::Command};

/// A cube of `size` per side with node `(r, g, b)` at `f` of its position.
fn cube(size: usize, f: impl Fn([f32; 3]) -> [f32; 3]) -> String {
    let last = (size - 1) as f32;
    let mut text = format!("TITLE \"Pack look\"\nLUT_3D_SIZE {size}\n");
    for b in 0..size {
        for g in 0..size {
            for r in 0..size {
                let [x, y, z] = f([r as f32 / last, g as f32 / last, b as f32 / last]);
                text += &format!("{x:.6} {y:.6} {z:.6}\n");
            }
        }
    }
    text
}

fn import(data: &Path, picks: &[&Path]) -> Result<(bool, String)> {
    let output = Command::new(env!("CARGO_BIN_EXE_rawmakase"))
        .arg("import-profiles")
        .args(picks)
        .env("RAWMAKASE_DATA_DIR", data)
        .output()?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok((output.status.success(), text))
}

/// A folder of cubes imports as looks, skipping the AppleDouble files zips carry
/// beside them; one the table can't hold is reported and leaves the rest importing;
/// importing again changes nothing.
#[test]
fn a_folder_of_cubes_imports_as_looks() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let pack = temp.path().join("Pack");
    fs::create_dir_all(pack.join("Sub"))?;
    fs::create_dir_all(pack.join("__MACOSX/Sub"))?;
    fs::write(
        pack.join("Warm.cube"),
        cube(17, |[r, g, b]| [r, g * 0.95, b * 0.8]),
    )?;
    // Sharp enough that resampling it onto 64 divisions shows.
    let sharp = |[r, g, b]: [f32; 3]| {
        let step = if b > 0.47 { r } else { 0.2 };
        [(12. * r).sin().abs(), step, g * g]
    };
    fs::write(pack.join("Sub/Big.cube"), cube(65, sharp))?;
    let apple_double = b"\x00\x05\x16\x07\x00\x02\x00\x00Mac OS X        \xff\xfe";
    fs::write(pack.join("._Warm.cube"), apple_double)?;
    fs::write(pack.join("__MACOSX/Sub/._Big.cube"), apple_double)?;
    fs::write(
        pack.join("Log.cube"),
        cube(2, |p| p).replace(
            "LUT_3D_SIZE 2",
            "LUT_3D_SIZE 2\nDOMAIN_MIN 0 0 0\nDOMAIN_MAX 4 4 4",
        ),
    )?;
    let data = temp.path().join("data");
    let (ok, out) = import(&data, &[&pack])?;
    // The refused cube fails the command, after the rest imported.
    assert!(!ok, "{out}");
    assert!(
        out.contains("Warm.cube: imported as look \"Warm\": exact"),
        "{out}"
    );
    assert!(
        out.contains("Big.cube: imported as look \"Big\": approximated")
            && out.contains("resampled from 65 divisions"),
        "{out}"
    );
    assert!(out.contains("Log.cube: the cube's domain"), "{out}");
    assert!(!out.contains("._"), "{out}");
    let library = data.join("camera-profiles");
    let mut files: Vec<_> = fs::read_dir(&library)?
        .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    files.sort();
    assert_eq!(files, ["Big.xmp", "Warm.xmp"]);
    let big = fs::read_to_string(library.join("Big.xmp"))?;
    assert!(big.contains(">Sub</rdf:li>") && big.contains(">Pack look</rdf:li>"));

    // Again, chosen by name: already imported, nothing written.
    let before = fs::read(library.join("Warm.xmp"))?;
    let (ok, out) = import(&data, &[&pack.join("Warm.cube")])?;
    assert!(ok, "{out}");
    assert!(out.contains("already imported as look \"Warm\""), "{out}");
    assert!(
        out.contains("Imported 0 profiles · 1 already imported"),
        "{out}"
    );
    assert_eq!(fs::read(library.join("Warm.xmp"))?, before);
    // The folder again with a new cube in it: each line names its own file, the
    // ones already imported beside the one written.
    fs::write(pack.join("Zest.cube"), cube(9, |[r, g, b]| [r * r, g, b]))?;
    let (_, out) = import(&data, &[&pack])?;
    assert!(
        out.contains("Zest.cube: imported as look \"Zest\""),
        "{out}"
    );
    assert!(
        out.contains("Warm.cube: already imported as look \"Warm\""),
        "{out}"
    );
    assert!(
        out.contains("Big.cube: already imported as look \"Big\""),
        "{out}"
    );
    // An AppleDouble file chosen by name is refused by what it is.
    let (ok, out) = import(&data, &[&pack.join("._Warm.cube")])?;
    assert!(!ok && out.contains("not text"), "{out}");
    Ok(())
}
