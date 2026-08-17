FROM ubuntu:22.04

# Avoid prompts during apt install
ENV DEBIAN_FRONTEND=noninteractive

# Install dependencies for UEFI OS development
RUN apt-get update && apt-get install -y \
    build-essential \
    nasm \
    mtools \
    dosfstools \
    gnu-efi \
    qemu-system-x86 \
    ovmf \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
