use rocket::data::{ByteUnit, ToByteUnit};

fn main() {
    let ten_meg = 10.mebibytes();
    println!("{:?}", ten_meg);
    println!("{}", ten_meg.as_u64());
    println!("{}", ten_meg.as_u64() == 10 * 1024 * 1024);
}
