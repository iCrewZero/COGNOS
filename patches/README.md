Place ANFS kernel patches here as `*.patch`.

`scripts/build_kernel.sh` applies every `patches/*.patch` in lexical order with
`patch -p1` before `olddefconfig` and `bindeb-pkg`.
