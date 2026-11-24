use std::fs;
use std::path::Path;

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

fn res_record(res_type: u16, res_id: u16, data: &[u8]) -> Vec<u8> {
    let mut header = Vec::new();
    header.extend_from_slice(&(data.len() as u32).to_le_bytes());
    header.extend_from_slice(&32u32.to_le_bytes());
    header.extend_from_slice(&0xFFFFu16.to_le_bytes());
    header.extend_from_slice(&res_type.to_le_bytes());
    header.extend_from_slice(&0xFFFFu16.to_le_bytes());
    header.extend_from_slice(&res_id.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0x1030u16.to_le_bytes());
    header.extend_from_slice(&0x0409u16.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&0u32.to_le_bytes());

    let mut out = header;
    out.extend_from_slice(data);
    pad4(&mut out);
    out
}

fn build_icon_res(ico_bytes: &[u8]) -> Option<Vec<u8>> {
    if ico_bytes.len() < 6 { return None; }
    let count = u16::from_le_bytes([ico_bytes[4], ico_bytes[5]]) as usize;
    if count == 0 || ico_bytes.len() < 6 + count * 16 { return None; }

    let mut res = res_record(0, 0, &[]);
    let mut grp = Vec::new();
    grp.extend_from_slice(&ico_bytes[0..6]);

    for i in 0..count {
        let off = 6 + i * 16;
        let entry = &ico_bytes[off..off + 16];
        let img_size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let img_offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if img_offset + img_size > ico_bytes.len() { return None; }

        let icon_id = (i + 1) as u16;
        grp.extend_from_slice(&entry[0..12]);
        grp.extend_from_slice(&icon_id.to_le_bytes());

        res.extend_from_slice(&res_record(3, icon_id, &ico_bytes[img_offset..img_offset + img_size]));
    }

    res.extend_from_slice(&res_record(14, 1, &grp));
    Some(res)
}

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(target_os = "windows")]
    {
        if let Ok(ico) = fs::read("assets/icon.ico") {
            if let Some(res_data) = build_icon_res(&ico) {
                let out_dir = std::env::var("OUT_DIR").unwrap();
                let res_path = Path::new(&out_dir).join("icon.res");
                if fs::write(&res_path, res_data).is_ok() {
                    println!("cargo:rustc-link-arg-bins={}", res_path.display());
                }
            }
        }
    }
}
