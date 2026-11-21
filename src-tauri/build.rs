fn main() {
    // tinyuz (MIT, github.com/sisong/tinyuz): the compressor the wireless fans' firmware expects for RGB data
    let vendor = std::path::Path::new("vendor");
    let tinyuz = vendor.join("tinyuz");
    let hdiff = vendor.join("HDiffPatch");
    let mut b = cc::Build::new();
    b.cpp(true)
        .opt_level(3)
        .define("NDEBUG", None)
        .define("_IS_USED_MULTITHREAD", "0")
        .include(vendor)
        .include(&tinyuz)
        .include(&hdiff)
        .warnings(false);
    for f in [
        vendor.join("tuz_wrapper.cpp"),
        tinyuz.join("compress/tuz_enc.cpp"),
        tinyuz.join("compress/tuz_enc_private/tuz_enc_clip.cpp"),
        tinyuz.join("compress/tuz_enc_private/tuz_enc_code.cpp"),
        tinyuz.join("compress/tuz_enc_private/tuz_enc_match.cpp"),
        tinyuz.join("compress/tuz_enc_private/tuz_sstring.cpp"),
        hdiff.join("libHDiffPatch/HDiff/private_diff/libdivsufsort/divsufsort.cpp"),
    ] {
        b.file(f);
    }
    b.compile("tinyuz");
    println!("cargo:rerun-if-changed=vendor");
    tauri_build::build()
}
