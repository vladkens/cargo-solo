#!/bin/bash
# The first build and the next of a crate with ring by cargo-solo, cargo-zigbuild and cross, for
# a web server (Debian and Alpine) and a CLI (Linux x86_64 and arm64, Windows), on this runner.
set -euo pipefail
W=$RUNNER_TEMP/bench
mkdir -p $W/m/src $W/shim
rustup -q toolchain install stable --profile minimal --component llvm-tools
rustup -q default stable
rustup -q target add x86_64-unknown-linux-gnu x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu
echo "host $(uname -m), $(nproc) cpus; $(clang --version | head -1); $(cross --version 2>/dev/null | head -1); $(cargo zigbuild --version)"
cargo install -q --locked --path . --root $W/solo

curl -sfLo $W/zig.tar.xz https://ziglang.org/download/0.17.1/zig-x86_64-linux-0.17.1.tar.xz
tar xf $W/zig.tar.xz -C $W
echo "zig: $(( $(stat -c %s $W/zig.tar.xz) / 1000000 )) MB download, $(du -sm $W/zig-x86_64-linux-0.17.1 | cut -f1) MB unpacked"
export PATH=$W/solo/bin:$W/zig-x86_64-linux-0.17.1:$PATH

# The real curl, logging the size of each file cargo-solo downloads.
cat > $W/shim/curl <<SH
#!/bin/bash
/usr/bin/curl "\$@"; rc=\$?
out=""; prev=""
for a in "\$@"; do [ "\$prev" = "-o" ] && out=\$a; prev=\$a; done
[ -n "\$out" ] && [ -f "\$out" ] && echo "\$(stat -c %s "\$out")" >> $W/downloads.log
exit \$rc
SH
chmod +x $W/shim/curl

cat > $W/m/Cargo.toml <<'TOML'
[package]
name = "m"
version = "0.1.0"
edition = "2021"

[dependencies]
ring = "0.17"
TOML
echo 'fn main() { println!("{:02x?}", &ring::digest::digest(&ring::digest::SHA256, b"hi").as_ref()[..4]); }' > $W/m/src/main.rs
cd $W/m && cargo fetch -q

ms() { echo $(( ($(date +%s%N) - $1) / 1000000 )); }
web="x86_64-unknown-linux-gnu x86_64-unknown-linux-musl"
cli="x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-gnu"
for ex in web cli; do
  ts=${!ex}
  args=$(for t in $ts; do echo -n " --target $t"; done)
  for run in cold warm; do
    [ $run = cold ] && rm -rf /tmp/zig-cache ~/.cache/zig
    cargo clean -q; s=$(date +%s%N)
    cargo zigbuild -q --release ${args//linux-gnu/linux-gnu.2.28}
    echo "| $ex | zigbuild | $run | $(ms $s) ms | cache $(du -sm ~/.cache/zig | cut -f1) MB |"
  done

  for run in cold warm; do
    rm -f $W/downloads.log
    [ $run = cold ] && rm -rf $W/solo-cache
    cargo clean -q; s=$(date +%s%N)
    PATH=$W/shim:$PATH CARGO_SOLO_CACHE=$W/solo-cache cargo solo build -q --release $args
    dl=$(awk '{s+=$1} END {printf "%.1f", s/1e6}' $W/downloads.log 2>/dev/null || echo 0)
    echo "| $ex | solo | $run | $(ms $s) ms | cache $(du -sm $W/solo-cache | cut -f1) MB, downloaded $dl MB |"
  done

  for run in cold warm; do
    [ $run = cold ] && docker images -q --filter 'reference=ghcr.io/cross-rs/*' | xargs -r docker rmi -f > /dev/null
    cargo clean -q; s=$(date +%s%N)
    for t in $ts; do cross build -q --release --target $t; done
    size=$(docker images --format '{{.Repository}}:{{.Tag}} {{.Size}}' | grep cross-rs | tr '\n' ',' )
    echo "| $ex | cross | $run | $(ms $s) ms | images $size |"
  done
done
