fn main() {
    println!("cargo:rerun-if-changed=res/prisma.ico");
    println!("cargo:rerun-if-changed=res/prisma.manifest");
    println!("cargo:rerun-if-changed=res/en.tsv");
    let mut res = winresource::WindowsResource::new();
    res.set_icon("res/prisma.ico");
    res.set_manifest_file("res/prisma.manifest");
    res.compile().expect("falha ao compilar recursos");
    // Windows N não traz o Media Foundation: só a gravação fica de fora, o resto abre.
    // O dbghelp só é preciso se o programa cair, o WinHTTP ao procurar atualização e o mscms na aba HDR.
    for dll in ["mfplat.dll", "mf.dll", "mfreadwrite.dll", "dbghelp.dll", "winhttp.dll", "mscms.dll"] {
        println!("cargo:rustc-link-arg=/DELAYLOAD:{dll}");
    }
    println!("cargo:rustc-link-arg=delayimp.lib");
    // As DLLs da abertura vêm só da System32: na versão portátil, um version.dll baixado para a
    // mesma pasta rodaria dentro do Prisma. As carregadas depois ficam com o `main`.
    println!("cargo:rustc-link-arg=/DEPENDENTLOADFLAG:0x800");
    build_hook();
}

/// O limitador é uma DLL própria, embutida no programa: tudo continua sendo um arquivo só.
fn build_hook() {
    println!("cargo:rerun-if-changed=hook/src/lib.rs");
    println!("cargo:rerun-if-changed=hook/Cargo.toml");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("hook");
    let cargo = std::env::var("CARGO").unwrap();
    let done = std::process::Command::new(cargo)
        .args(["build", "--release", "--locked", "--manifest-path", "hook/Cargo.toml", "--target-dir"])
        .arg(&out)
        .status()
        .expect("falha ao chamar o cargo");
    assert!(done.success(), "falha ao compilar o limitador");
}
