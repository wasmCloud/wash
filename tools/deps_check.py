import os
import shutil

cargo = shutil.which('cargo')
if cargo is None:
    print('cargo not found. Please install Rust from https://rustup.rs/')
    exit(1)

go = shutil.which('go')
if go is None:
    print('go not found. Please install it from https://golang.org/')
    exit(1)

tinygo = shutil.which('tinygo')
if tinygo is None:
    print('tinygo not found. Please install it from https://tinygo.org/')
    exit(1)

targets = os.popen('rustup target list').read()
if "wasm32-unknown-unknown" not in targets:
    print('Rust wasm32-unknown-unknown target not found. Installing..."')
    os.system('rustup target add wasm32-unknown-unknown')

os.system('cargo install cargo-nextest --locked')