import os
import shutil
import subprocess

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

targets = subprocess.run("rustup target list", shell=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True).stdout
if "wasm32-unknown-unknown" not in targets:
    print('Rust wasm32-unknown-unknown target not found. Installing..."')
    os.system('rustup target add wasm32-unknown-unknown')

nextest_output = subprocess.run("cargo nextest --version", shell=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True).stdout
if "error: no such command" in nextest_output:
    print('cargo nextest not found. Installing..."')
    os.system('cargo install cargo-nextest --locked')

print("All dependencies are installed!")