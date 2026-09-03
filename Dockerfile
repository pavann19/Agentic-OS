FROM ubuntu:22.04

# Avoid prompts during apt install
ENV DEBIAN_FRONTEND=noninteractive

# Install dependencies for UEFI OS development.
# lld/llvm are needed as the Rust kernel's linker (rust-lld) and for the
# custom bare-metal target's codegen tools; curl/ca-certificates are needed
# to install rustup (build-essential/nasm/mtools/etc. stay for the UEFI
# bootloader, which remains C per ADR-002 — only new kernel code moves to Rust).
RUN apt-get update && apt-get install -y \
    build-essential \
    nasm \
    mtools \
    dosfstools \
    gnu-efi \
    qemu-system-x86 \
    ovmf \
    curl \
    ca-certificates \
    lld \
    llvm \
    && rm -rf /var/lib/apt/lists/*

# Rust nightly is required for -Zbuild-std (building core/compiler_builtins
# from source for our custom bare-metal target) and rust-src for that build.
ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | \
    sh -s -- -y --default-toolchain nightly --profile minimal \
    --component rust-src --component llvm-tools

WORKDIR /workspace
