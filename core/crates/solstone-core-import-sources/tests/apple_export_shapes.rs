//! The shapes an Apple export comes in, and the zip64 ends a
//! file can claim. Synthetic data only; the shapes come from what's known of an
//! export, as no real one is to hand.
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use solstone_core_import_sources::apple_export::{
    AppleItem, detect_apple_source, scan_apple_export,
};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "apple-export-shapes-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const DTD: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE HealthData [
<!-- HealthKit Export Version: 14 -->
<!ELEMENT HealthData (ExportDate,Me,(Record|Correlation|Workout|ActivitySummary|ClinicalRecord|Audiogram|VisionPrescription)*)>
<!ATTLIST HealthData
  locale CDATA #REQUIRED
>
<!ELEMENT ExportDate EMPTY>
<!ATTLIST ExportDate
  value CDATA #REQUIRED
>
<!ELEMENT Me EMPTY>
<!ATTLIST Me
  HKCharacteristicTypeIdentifierDateOfBirth         CDATA #REQUIRED
  HKCharacteristicTypeIdentifierBiologicalSex       CDATA #REQUIRED
>
<!ELEMENT Record ((MetadataEntry|HeartRateVariabilityMetadataList)*)>
<!ATTLIST Record
  type          CDATA #REQUIRED
  unit          CDATA #IMPLIED
  value         CDATA #IMPLIED
  sourceName    CDATA #REQUIRED
  sourceVersion CDATA #IMPLIED
  device        CDATA #IMPLIED
  creationDate  CDATA #IMPLIED
  startDate     CDATA #REQUIRED
  endDate       CDATA #REQUIRED
>
<!-- Note: Any Records that appear as children of a correlation also appear as top-level records in this document. -->
<!ELEMENT Correlation ((MetadataEntry|Record)*)>
<!ATTLIST Correlation
  type          CDATA #REQUIRED
  startDate     CDATA #REQUIRED
>
<!ELEMENT Workout ((MetadataEntry|WorkoutEvent|WorkoutRoute|WorkoutStatistics)*)>
<!ENTITY % foo "a > b">
<!ATTLIST Quoted x CDATA "it's > fine">
]>
"#;

fn rec(start: &str, val: &str) -> String {
    format!(
        r#"<Record type="HKQuantityTypeIdentifierHeartRate" sourceName="Sam’s Apple&#xA0;Watch" sourceVersion="1" device="&lt;&lt;HKDevice: 0x28&gt;, name:Apple Watch&gt;" unit="count/min" creationDate="{start}" startDate="{start}" endDate="{start}" value="{val}"/>"#
    )
}

fn body() -> String {
    let mut text = String::new();
    text.push_str(DTD);
    text.push_str(" <HealthData locale=\"en_US\">\n <ExportDate value=\"2026-10-12 08:00:00 -0600\"/>\n <Me HKCharacteristicTypeIdentifierDateOfBirth=\"\" HKCharacteristicTypeIdentifierBiologicalSex=\"x\"/>\n");
    text.push_str(&rec("2026-10-10 06:31:10 -0600", "70"));
    text.push('\n');
    // record with hrv children
    text.push_str(r#"<Record type="HKQuantityTypeIdentifierHeartRateVariabilitySDNN" sourceName="w" unit="ms" startDate="2026-10-10 06:40:00 -0600" endDate="2026-10-10 06:41:00 -0600" value="50">
  <MetadataEntry key="HKMetadataKeySyncIdentifier" value="abc"/>
  <HeartRateVariabilityMetadataList>
   <InstantaneousBeatsPerMinute bpm="61" time="6:40:10.62 AM"/>
  </HeartRateVariabilityMetadataList>
 </Record>
"#);
    text.push_str(r#"<Correlation type="HKCorrelationTypeIdentifierBloodPressure" sourceName="c" startDate="2026-10-10 07:00:00 -0600" endDate="2026-10-10 07:00:00 -0600">
  <MetadataEntry key="k" value="v"/>
  <Record type="HKQuantityTypeIdentifierBloodPressureSystolic" sourceName="c" unit="mmHg" startDate="2026-10-10 07:00:00 -0600" endDate="2026-10-10 07:00:00 -0600" value="120"/>
  <Record type="HKQuantityTypeIdentifierBloodPressureDiastolic" sourceName="c" unit="mmHg" startDate="2026-10-10 07:00:00 -0600" endDate="2026-10-10 07:00:00 -0600" value="80"/>
 </Correlation>
"#);
    text.push_str(r#"<Workout workoutActivityType="HKWorkoutActivityTypeRunning" duration="10" durationUnit="min" sourceName="w" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:10:00 -0600">
  <MetadataEntry key="HKIndoorWorkout" value="0"/>
  <WorkoutEvent type="HKWorkoutEventTypeSegment" date="2026-10-10 08:01:00 -0600" duration="1" durationUnit="min"/>
  <WorkoutStatistics type="HKQuantityTypeIdentifierDistanceWalkingRunning" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:10:00 -0600" sum="1.2" unit="km"/>
  <WorkoutActivity uuid="U" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:10:00 -0600" duration="10" durationUnit="min">
   <WorkoutEvent type="x" date="2026-10-10 08:01:00 -0600"/>
   <MetadataEntry key="a" value="b"/>
  </WorkoutActivity>
  <WorkoutRoute sourceName="w" startDate="2026-10-10 08:00:00 -0600" endDate="2026-10-10 08:10:00 -0600">
   <MetadataEntry key="HKMetadataKeySyncVersion" value="2"/>
   <FileReference path="/workout-routes/route_2026-10-10_8.00am.gpx"/>
  </WorkoutRoute>
 </Workout>
 <ActivitySummary dateComponents="2026-10-10" activeEnergyBurned="1"/>
 <ClinicalRecord type="x" identifier="y" sourceName="z" sourceURL="u" fhirVersion="4" receivedDate="d" resourceFilePath="/clinical-records/a.json"/>
 <Audiogram type="x" sourceName="y" startDate="2026-10-10 09:00:00 -0600" endDate="2026-10-10 09:00:00 -0600"><SensitivityPoint frequencyValue="1"/></Audiogram>
"#);
    text
}

fn dir_with(tmp: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let dir = tmp.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("export.xml"), bytes).unwrap();
    dir
}

/// Records, workouts, refusals by stage and elements not read.
type Scanned = (usize, usize, Vec<&'static str>, Vec<String>);

fn items(source: &Path) -> Result<Scanned, String> {
    let mut records = 0;
    let mut workouts = 0;
    let mut refused = Vec::new();
    let mut notread = Vec::new();
    scan_apple_export(source, &mut |item| match item {
        AppleItem::Record(_) => records += 1,
        AppleItem::Workout(_) => workouts += 1,
        AppleItem::Refused(stage) => refused.push(stage),
        AppleItem::NotRead(name) => notread.push(name),
    })
    .map_err(|error| error.stage().to_owned())?;
    Ok((records, workouts, refused, notread))
}

#[test]
fn realistic_exports_scan_whole() {
    let tmp = TempDir::new();
    let close = "</HealthData>";
    for (name, tail) in [
        ("plain", format!("{close}\n")),
        ("no-newline", close.to_owned()),
        ("crlf", format!("{close}\r\n\r\n")),
        (
            "trailing-text",
            format!("{close}\n trailing words stuff \n"),
        ),
        ("trailing-nul", format!("{close}\0\0\0\0")),
        ("trailing-comment", format!("{close}\n<!-- bye -->\n")),
        ("trailing-pi", format!("{close}\n<?pi x?>\n")),
    ] {
        let bytes = format!("{}{tail}", body());
        let dir = dir_with(tmp.0.as_path(), name, bytes.as_bytes());
        let (r, w, refused, _) = items(&dir).unwrap();
        assert_eq!((r, w, refused.len()), (4, 1, 0), "{name}");
    }
    // A BOM, too.
    let bytes = [
        b"\xEF\xBB\xBF".as_slice(),
        body().as_bytes(),
        b"</HealthData>\n",
    ]
    .concat();
    let dir = dir_with(tmp.0.as_path(), "bom", &bytes);
    let (r, w, refused, _) = items(&dir).unwrap();
    assert_eq!((r, w, refused.len()), (4, 1, 0), "bom");
}

fn zip_of(path: &Path, members: &[(&str, &[u8])]) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in members {
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn realistic_zip_with_cda_and_friends() {
    let tmp = TempDir::new();
    let xml = format!("{}</HealthData>\n", body());
    for (name, members) in [
        (
            "cda-first",
            vec![
                (
                    "apple_health_export/export_cda.xml",
                    b"<ClinicalDocument/>".as_slice(),
                ),
                ("apple_health_export/export.xml", xml.as_bytes()),
                (
                    "apple_health_export/workout-routes/route_1.gpx",
                    b"<gpx/>".as_slice(),
                ),
            ],
        ),
        (
            "cda-last",
            vec![
                ("apple_health_export/export.xml", xml.as_bytes()),
                (
                    "apple_health_export/export_cda.xml",
                    b"<ClinicalDocument>not closed".as_slice(),
                ),
            ],
        ),
        (
            "macosx",
            vec![
                (
                    "__MACOSX/apple_health_export/._export.xml",
                    b"\x00\x05\x16\x07garbage".as_slice(),
                ),
                ("apple_health_export/export.xml", xml.as_bytes()),
            ],
        ),
        (
            "nested",
            vec![
                ("Downloads/apple_health_export/export.xml", xml.as_bytes()),
                (
                    "Downloads/apple_health_export/export_cda.xml",
                    b"<x/>".as_slice(),
                ),
            ],
        ),
    ] {
        let path = tmp.0.as_path().join(format!("{name}.zip"));
        zip_of(&path, &members);
        assert_eq!(items(&path).unwrap().0, 4, "{name}");
    }
}

#[test]
fn close_rule_table() {
    let tmp = TempDir::new();
    let r = rec("2026-10-10 06:31:10 -0600", "70");
    let r2 = rec("2026-10-10 06:32:10 -0600", "71");
    let cases: Vec<(&str, String)> = vec![
        (
            "second-root-cut",
            format!("<HealthData>{r}</HealthData><HealthData>{r2}"),
        ),
        (
            "second-root-closed",
            format!("<HealthData>{r}</HealthData><HealthData>{r2}</HealthData>"),
        ),
        (
            "record-after-close",
            format!("<HealthData>{r}</HealthData>{r2}"),
        ),
        (
            "open-element-after-close",
            format!("<HealthData>{r}</HealthData><Foo>"),
        ),
        (
            "open-correlation-after-close",
            format!("<HealthData>{r}</HealthData><Correlation>{r2}"),
        ),
        (
            "open-record-after-close",
            format!(
                "<HealthData>{r}</HealthData><Record type=\"a\" startDate=\"2026-10-10 06:31:10 -0600\">"
            ),
        ),
        (
            "nested-healthdata-cut",
            format!("<HealthData>{r}<HealthData></HealthData>{r2}"),
        ),
        ("empty-then-records", format!("<HealthData/>{r}")),
        ("wrapped-cut", format!("<Foo><HealthData>{r}</HealthData>")),
        (
            "in-correlation-cut",
            format!("<HealthData>{r}<Correlation>{r2}"),
        ),
        (
            "in-workout-child-cut",
            format!(
                "<HealthData>{r}<Workout workoutActivityType=\"x\" startDate=\"2026-10-10 06:31:10 -0600\" endDate=\"2026-10-10 06:39:10 -0600\"><WorkoutRoute>"
            ),
        ),
        (
            "mismatched",
            format!("<HealthData>{r}<Correlation></HealthData>"),
        ),
        (
            "stray-end-after-close",
            format!("<HealthData>{r}</HealthData></HealthData>"),
        ),
        ("cut-close-tag", format!("<HealthData>{r}</HealthDa")),
        (
            "bang-garbage",
            format!("<HealthData>{r}<!garbage>{r2}</HealthData>"),
        ),
        ("no-root-just-records", r.clone()),
        ("case", format!("<healthdata>{r}</healthdata>")),
    ];
    for (name, text) in cases {
        let dir = dir_with(tmp.0.as_path(), name, text.as_bytes());
        println!("{name:32} scan={:?}", items(&dir));
    }
}

#[test]
fn bad_attribute_table() {
    let tmp = TempDir::new();
    let r = rec("2026-10-10 06:31:10 -0600", "70");
    let r2 = rec("2026-10-10 06:32:10 -0600", "71");
    let bad = r#"type="a" type="b" startDate="2026-10-10 06:31:10 -0600""#;
    let cases: Vec<(&str, String)> = vec![
        (
            "bad-empty",
            format!("<HealthData><Record {bad}/>{r}</HealthData>"),
        ),
        (
            "bad-start-with-kids",
            format!(
                "<HealthData><Record {bad}><MetadataEntry key=\"k\" value=\"v\"/><HeartRateVariabilityMetadataList><I b=\"1\"/></HeartRateVariabilityMetadataList></Record>{r}</HealthData>"
            ),
        ),
        (
            "bad-in-correlation",
            format!(
                "<HealthData><Correlation type=\"c\"><Record {bad}/>{r}</Correlation>{r2}</HealthData>"
            ),
        ),
        (
            "bad-start-in-correlation",
            format!(
                "<HealthData><Correlation type=\"c\"><Record {bad}><MetadataEntry key=\"k\" value=\"v\"/></Record>{r}</Correlation>{r2}</HealthData>"
            ),
        ),
        (
            "bad-correlation-attr",
            format!("<HealthData><Correlation {bad}>{r}</Correlation>{r2}</HealthData>"),
        ),
        (
            "bad-healthdata-attr",
            format!("<HealthData {bad}>{r}</HealthData>"),
        ),
        (
            "bad-workout",
            format!(
                "<HealthData><Workout {bad}><WorkoutEvent a=\"1\"/><WorkoutRoute><MetadataEntry key=\"a\" value=\"b\"/></WorkoutRoute></Workout>{r}</HealthData>"
            ),
        ),
        (
            "bad-child-first",
            format!(
                "<HealthData><Workout workoutActivityType=\"x\" startDate=\"2026-10-10 06:31:10 -0600\" endDate=\"2026-10-10 06:39:10 -0600\"><WorkoutEvent a=\"1\" a=\"2\"/><WorkoutEvent b=\"1\"/><Foo/></Workout>{r}</HealthData>"
            ),
        ),
        (
            "bad-child-with-kids",
            format!(
                "<HealthData><Workout workoutActivityType=\"x\" startDate=\"2026-10-10 06:31:10 -0600\" endDate=\"2026-10-10 06:39:10 -0600\"><WorkoutStatistics a=\"1\" a=\"2\"><Foo><Bar/></Foo></WorkoutStatistics><WorkoutEvent b=\"1\"/></Workout>{r}</HealthData>"
            ),
        ),
        (
            "bad-entity",
            format!(
                "<HealthData><Record type=\"a&bogus;\" startDate=\"2026-10-10 06:31:10 -0600\"/>{r}</HealthData>"
            ),
        ),
        (
            "bad-unquoted",
            format!(
                "<HealthData><Record type=a startDate=\"2026-10-10 06:31:10 -0600\"/>{r}</HealthData>"
            ),
        ),
        (
            "bad-grandchild-unread",
            format!(
                "<HealthData><Workout workoutActivityType=\"x\" startDate=\"2026-10-10 06:31:10 -0600\" endDate=\"2026-10-10 06:39:10 -0600\"><WorkoutRoute><MetadataEntry a=\"1\" a=\"2\"/></WorkoutRoute></Workout>{r}</HealthData>"
            ),
        ),
        (
            "open-quote-swallow",
            format!("<HealthData><Record type=\"a startDate=\"2026-10-10\"/>{r}{r2}</HealthData>"),
        ),
        (
            "open-quote-gt-swallow",
            format!(
                "<HealthData><Record type=\"a/>\n{r}\n<Record type=\"x>y\" value=\"z\"/>{r2}</HealthData>"
            ),
        ),
        ("bad-then-cut", format!("<HealthData><Record {bad}>{r}")),
        (
            "bad-then-close-name-mismatch",
            format!("<HealthData><Record {bad}></HealthData>"),
        ),
    ];
    for (name, text) in cases {
        let dir = dir_with(tmp.0.as_path(), name, text.as_bytes());
        println!("{name:32} scan={:?}", items(&dir));
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A plain end record pointing at a zip64 record, with chosen fields left real.
fn zip64_real(path: &Path, xml: &[u8], hole: u64, entries_max: bool, size_max: bool) {
    use std::io::{Seek, SeekFrom};
    let name = b"apple_health_export/export.xml";
    let mut file = fs::File::create(path).unwrap();
    // A sparse padding gap before the member, as a large earlier member would leave.
    file.seek(SeekFrom::Start(hole)).unwrap();
    let local = file.stream_position().unwrap();
    let crc = crc32(xml);
    let mut header = Vec::new();
    header.extend_from_slice(b"PK\x03\x04");
    header.extend_from_slice(&20_u16.to_le_bytes());
    header.extend_from_slice(&0x0800_u16.to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(&crc.to_le_bytes());
    header.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    header.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    header.extend_from_slice(&(name.len() as u16).to_le_bytes());
    header.extend_from_slice(&0_u16.to_le_bytes());
    header.extend_from_slice(name);
    file.write_all(&header).unwrap();
    file.write_all(xml).unwrap();
    let central_at = file.stream_position().unwrap();
    let mut central = Vec::new();
    central.extend_from_slice(b"PK\x01\x02");
    central.extend_from_slice(&45_u16.to_le_bytes());
    central.extend_from_slice(&45_u16.to_le_bytes());
    central.extend_from_slice(&0x0800_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&crc.to_le_bytes());
    central.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    central.extend_from_slice(&(xml.len() as u32).to_le_bytes());
    central.extend_from_slice(&(name.len() as u16).to_le_bytes());
    central.extend_from_slice(&12_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u16.to_le_bytes());
    central.extend_from_slice(&0_u32.to_le_bytes());
    central.extend_from_slice(&u32::MAX.to_le_bytes());
    central.extend_from_slice(name);
    central.extend_from_slice(&1_u16.to_le_bytes());
    central.extend_from_slice(&8_u16.to_le_bytes());
    central.extend_from_slice(&local.to_le_bytes());
    file.write_all(&central).unwrap();
    let record_at = file.stream_position().unwrap();
    let mut end = Vec::new();
    end.extend_from_slice(b"PK\x06\x06");
    end.extend_from_slice(&44_u64.to_le_bytes());
    end.extend_from_slice(&45_u16.to_le_bytes());
    end.extend_from_slice(&45_u16.to_le_bytes());
    end.extend_from_slice(&0_u32.to_le_bytes());
    end.extend_from_slice(&0_u32.to_le_bytes());
    end.extend_from_slice(&1_u64.to_le_bytes());
    end.extend_from_slice(&1_u64.to_le_bytes());
    end.extend_from_slice(&(central.len() as u64).to_le_bytes());
    end.extend_from_slice(&central_at.to_le_bytes());
    end.extend_from_slice(b"PK\x06\x07");
    end.extend_from_slice(&0_u32.to_le_bytes());
    end.extend_from_slice(&record_at.to_le_bytes());
    end.extend_from_slice(&1_u32.to_le_bytes());
    end.extend_from_slice(b"PK\x05\x06");
    end.extend_from_slice(&[0_u8; 4]);
    let entries = if entries_max { u16::MAX } else { 1 };
    end.extend_from_slice(&entries.to_le_bytes());
    end.extend_from_slice(&entries.to_le_bytes());
    let size = if size_max {
        u32::MAX
    } else {
        central.len() as u32
    };
    end.extend_from_slice(&size.to_le_bytes());
    end.extend_from_slice(&u32::MAX.to_le_bytes());
    end.extend_from_slice(&[0_u8; 2]);
    file.write_all(&end).unwrap();
}

// Sparse on Unix; Windows would write the 5 GiB.
#[cfg(unix)]
#[test]
fn a_real_shaped_zip_over_4gib_reads_end_to_end() {
    let tmp = TempDir::new();
    let xml = format!("{}</HealthData>\n", body());
    let hole = 5_u64 * 1024 * 1024 * 1024;
    for (name, entries_max, size_max) in [
        ("all-max", true, true),
        ("offset-only", false, false),
        ("size-offset", false, true),
    ] {
        let path = tmp.0.join(format!("{name}.zip"));
        zip64_real(&path, xml.as_bytes(), hole, entries_max, size_max);
        println!("{name}: len={}", fs::metadata(&path).unwrap().len());
        println!(
            "  detect={:?}",
            detect_apple_source(&path).map_err(|e| e.stage())
        );
        println!("  scan={:?}", items(&path).map(|i| (i.0, i.1)));
        assert_eq!(items(&path).unwrap().0, 4, "{name}");
    }
}

/// Untrusted bytes at the end of a file: nothing panics, and every refusal is a stage.
#[test]
fn crafted_zip64_ends_are_refused_without_panic() {
    let tmp = TempDir::new();
    let path = tmp.0.join("x.zip");
    let eocd = |entries: u16, size: u32, offset: u32| {
        let mut end = Vec::new();
        end.extend_from_slice(b"PK\x05\x06");
        end.extend_from_slice(&[0_u8; 4]);
        end.extend_from_slice(&entries.to_le_bytes());
        end.extend_from_slice(&entries.to_le_bytes());
        end.extend_from_slice(&size.to_le_bytes());
        end.extend_from_slice(&offset.to_le_bytes());
        end.extend_from_slice(&[0_u8; 2]);
        end
    };
    let locator = |offset: u64, total: u32| {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x06\x07");
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
        bytes.extend_from_slice(&total.to_le_bytes());
        bytes
    };
    let record = |entries: u64, on_disk: u64, size: u64, at: u64| {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"PK\x06\x06");
        bytes.extend_from_slice(&44_u64.to_le_bytes());
        bytes.extend_from_slice(&[0_u8; 4]);
        bytes.extend_from_slice(&[0_u8; 8]);
        bytes.extend_from_slice(&on_disk.to_le_bytes());
        bytes.extend_from_slice(&entries.to_le_bytes());
        bytes.extend_from_slice(&size.to_le_bytes());
        bytes.extend_from_slice(&at.to_le_bytes());
        bytes
    };
    let max_e = eocd(u16::MAX, u32::MAX, u32::MAX);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("eocd-only", max_e.clone()),
        ("short-prefix", [vec![0_u8; 10], max_e.clone()].concat()),
        (
            "locator-offset-max",
            [locator(u64::MAX, 1), max_e.clone()].concat(),
        ),
        (
            "locator-overflow",
            [locator(u64::MAX - 10, 1), max_e.clone()].concat(),
        ),
        (
            "locator-at-self",
            [vec![0_u8; 56], locator(0, 1), max_e.clone()].concat(),
        ),
        (
            "locator-disks-0",
            [record(0, 0, 0, 0), locator(0, 0), max_e.clone()].concat(),
        ),
        (
            "record-size-max",
            [record(1, 1, u64::MAX, 0), locator(0, 1), max_e.clone()].concat(),
        ),
        (
            "record-offset-max",
            [record(1, 1, 1, u64::MAX), locator(0, 1), max_e.clone()].concat(),
        ),
        (
            "record-both-overflow",
            [
                record(1, 1, u64::MAX, u64::MAX),
                locator(0, 1),
                max_e.clone(),
            ]
            .concat(),
        ),
        (
            "record-entries-max",
            [
                record(u64::MAX, u64::MAX, 0, 0),
                locator(0, 1),
                max_e.clone(),
            ]
            .concat(),
        ),
        (
            "record-disk-mismatch",
            [record(1, 2, 0, 0), locator(0, 1), max_e.clone()].concat(),
        ),
        (
            "zero-entries",
            [record(0, 0, 0, 0), locator(0, 1), max_e.clone()].concat(),
        ),
        ("plain-entries-max-no-zip64", eocd(u16::MAX, 0, 0)),
    ];
    for (name, bytes) in cases {
        fs::write(&path, &bytes).unwrap();
        println!(
            "{name:28} detect={:?} scan={:?}",
            detect_apple_source(&path).map_err(|e| e.stage()),
            items(&path)
        );
    }
}

const SHORT_EXPORT: &[u8] =
    b"<HealthData><Record type=\"a\" startDate=\"2026-01-02 00:00:00 +0000\"/></HealthData>";

#[cfg(unix)]
#[test]
fn an_intermediate_source_symlink_is_refused() {
    use std::os::unix::fs::symlink;

    let tmp = TempDir::new();
    let source = tmp.0.join("source");
    let outside = tmp.0.join("outside/apple_health_export");
    fs::create_dir(&source).unwrap();
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("export.xml"), SHORT_EXPORT).unwrap();
    symlink(&outside, source.join("apple_health_export")).unwrap();

    let detection = detect_apple_source(&source).expect_err("detection must reject the symlink");
    assert_eq!(detection.stage(), "source_symlink");
    assert_eq!(items(&source).unwrap_err(), "source_symlink");
}

#[cfg(unix)]
#[test]
fn a_fifo_named_like_an_archive_is_refused_without_blocking() {
    use nix::sys::stat::Mode;
    use nix::unistd::mkfifo;

    let tmp = TempDir::new();
    let source = tmp.0.join("synthetic.zip");
    mkfifo(&source, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();

    let detection = detect_apple_source(&source).expect_err("FIFO detection must fail closed");
    assert_eq!(detection.stage(), "source_symlink");
    assert_eq!(items(&source).unwrap_err(), "archive");
}
