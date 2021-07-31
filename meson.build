project(
    'unweave',
    version: '1.0.0',
    meson_version : '>= 0.56')


build_dir = meson.current_build_dir()
cargo = find_program('cargo')
if get_option('strip')
    strip = find_program('strip')
else
    cp = find_program('cp')
endif

if get_option('manpage')
    pandoc = find_program('pandoc', required: false)
    if not pandoc.found()
        error('pandoc is required to build the manpage, use -Dmanpage=false to disable')
    endif
endif

buildtype = get_option('buildtype')
if buildtype == 'debug' or buildtype == 'plain'
    cargo_target = 'debug'
    cargo_cmd = [cargo, 'build', '--target-dir', build_dir]
elif buildtype == 'debugoptimized'
    cargo_target = 'debugoptimized'
    cargo_cmd = [cargo, 'build', '--profile=debugoptimized', '--target-dir', build_dir]
elif buildtype == 'debugoptimized'
    cargo_target = 'release'
    cargo_cmd = [cargo, 'build', '--release', '--target-dir', build_dir]
else
    error('Unsupported buildtype: ' + buildtype)
endif

unweave_bin = custom_target(
    'unweave-bin',
    console: true,
    output: 'unweave.stamp',
    command: cargo_cmd,
)

if get_option('strip')
    cp_cmd = [strip, '-o', '@OUTPUT@', build_dir / cargo_target / 'unweave']
else
    cp_cmd = [cp, build_dir / cargo_target / 'unweave', '@OUTPUT@']
endif

unweave = custom_target(
    'unweave',
    build_by_default: true,
    install: true,
    install_dir: get_option('bindir'),
    depends: unweave_bin,
    output: 'unweave',
    command: cp_cmd,
)

if get_option('manpage')
    man = custom_target(
        'unweave.1',
        build_by_default: true,
        install: true,
        install_dir: get_option('mandir') / 'man1',
        input: 'doc/unweave.1.md',
        output: 'unweave.1',
        command: [pandoc, '--standalone', '--to', 'man', '@INPUT@', '-o', '@OUTPUT@']
    )
endif
