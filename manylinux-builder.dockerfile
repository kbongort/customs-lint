FROM quay.io/pypa/manylinux_2_28_x86_64

# Pre-install Rust and Maturin
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
ENV PATH="/root/.cargo/bin:${PATH}"
RUN pipx install maturin

WORKDIR /io