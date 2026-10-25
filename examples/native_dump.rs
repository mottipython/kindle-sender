use std::path::Path;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let src = Path::new(&a[1]);
    let ext = format!(".{}", src.extension().unwrap().to_str().unwrap());
    kindle_sender::native::convert(src, Path::new(&a[2]), &ext).unwrap();
}
