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
}
