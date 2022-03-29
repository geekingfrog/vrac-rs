use std::error::Error;

use rocket::data::{ByteUnit, ToByteUnit};

use scrypt::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Scrypt,
};

fn main() -> Result<(), Box<dyn Error>>{
    let password = b"coucou";
    let salt = SaltString::generate(&mut OsRng);
    let hash = Scrypt.hash_password(password, &salt)?.to_string();
    println!("phc: {}", hash);
    // let parsed_hash = PasswordHash::new(&hash);
    // println!("{parsed_hash:?}");
    // let result = Scrypt.verify_password(b"hunter2", &parsed_hash?);
    // println!("{result:?}");
    Ok(())
}
