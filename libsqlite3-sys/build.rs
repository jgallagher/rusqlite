use std::env;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir).join("bindgen.rs");
    if cfg!(feature = "in_gecko") {
        // When inside mozilla-central, we are included into the build with
        // sqlite3.o directly, so we don't want to provide any linker arguments.
        std::fs::copy("sqlite3/bindgen_bundled_version.rs", out_path)
            .expect("Could not copy bindings to output directory");
        return;
    }
    if cfg!(feature = "sqlcipher") {
        if cfg!(any(
            feature = "bundled",
            all(windows, feature = "bundled-windows")
        )) {
            println!(
                "cargo:warning=Builds with bundled SQLCipher are not supported. Searching for SQLCipher to link against. \
                 This can lead to issues if your version of SQLCipher is not up to date!");
        }
        build_linked::main(&out_dir, &out_path)
    } else if cfg!(feature = "loadable_extension") {
        build_loadable_extension::main(&out_dir, &out_path)
    } else {
        // This can't be `cfg!` without always requiring our `mod build_bundled` (and
        // thus `cc`)
        #[cfg(any(feature = "bundled", all(windows, feature = "bundled-windows")))]
        {
            build_bundled::main(&out_dir, &out_path)
        }
        #[cfg(not(any(feature = "bundled", all(windows, feature = "bundled-windows"))))]
        {
            build_linked::main(&out_dir, &out_path)
        }
    }
}

#[cfg(any(feature = "bundled", all(windows, feature = "bundled-windows")))]
mod build_bundled {
    use std::env;
    use std::path::Path;

    pub fn main(out_dir: &str, out_path: &Path) {
        if cfg!(feature = "sqlcipher") {
            // This is just a sanity check, the top level `main` should ensure this.
            panic!("Builds with bundled SQLCipher are not supported");
        }

        use super::{bindings, header_file, HeaderLocation};
        let header = HeaderLocation::FromPath(format!("sqlite3/{}", header_file()));
        bindings::write_to_out_dir(header, out_path);

        let mut cfg = cc::Build::new();
        cfg.file("sqlite3/sqlite3.c")
            .flag("-DSQLITE_CORE")
            .flag("-DSQLITE_DEFAULT_FOREIGN_KEYS=1")
            .flag("-DSQLITE_ENABLE_API_ARMOR")
            .flag("-DSQLITE_ENABLE_COLUMN_METADATA")
            .flag("-DSQLITE_ENABLE_DBSTAT_VTAB")
            .flag("-DSQLITE_ENABLE_FTS3")
            .flag("-DSQLITE_ENABLE_FTS3_PARENTHESIS")
            .flag("-DSQLITE_ENABLE_FTS5")
            .flag("-DSQLITE_ENABLE_JSON1")
            .flag("-DSQLITE_ENABLE_LOAD_EXTENSION=1")
            .flag("-DSQLITE_ENABLE_MEMORY_MANAGEMENT")
            .flag("-DSQLITE_ENABLE_RTREE")
            .flag("-DSQLITE_ENABLE_STAT2")
            .flag("-DSQLITE_ENABLE_STAT4")
            .flag("-DSQLITE_SOUNDEX")
            .flag("-DSQLITE_THREADSAFE=1")
            .flag("-DSQLITE_USE_URI")
            .flag("-DHAVE_USLEEP=1")
            .flag("-D_POSIX_THREAD_SAFE_FUNCTIONS") // cross compile with MinGW
            .warnings(false);

        if cfg!(feature = "with-asan") {
            cfg.flag("-fsanitize=address");
        }

        // Older versions of visual studio don't support c99 (including isnan), which
        // causes a build failure when the linker fails to find the `isnan`
        // function. `sqlite` provides its own implmentation, using the fact
        // that x != x when x is NaN.
        //
        // There may be other platforms that don't support `isnan`, they should be
        // tested for here.
        if cfg!(target_env = "msvc") {
            use cc::windows_registry::{find_vs_version, VsVers};
            let vs_has_nan = match find_vs_version() {
                Ok(ver) => ver != VsVers::Vs12,
                Err(_msg) => false,
            };
            if vs_has_nan {
                cfg.flag("-DHAVE_ISNAN");
            }
        } else {
            cfg.flag("-DHAVE_ISNAN");
        }
        if cfg!(not(target_os = "windows")) {
            cfg.flag("-DHAVE_LOCALTIME_R");
        }
        if cfg!(feature = "unlock_notify") {
            cfg.flag("-DSQLITE_ENABLE_UNLOCK_NOTIFY");
        }
        if cfg!(feature = "preupdate_hook") {
            cfg.flag("-DSQLITE_ENABLE_PREUPDATE_HOOK");
        }
        if cfg!(feature = "session") {
            cfg.flag("-DSQLITE_ENABLE_SESSION");
        }

        if let Ok(limit) = env::var("SQLITE_MAX_VARIABLE_NUMBER") {
            cfg.flag(&format!("-DSQLITE_MAX_VARIABLE_NUMBER={}", limit));
        }
        println!("cargo:rerun-if-env-changed=SQLITE_MAX_VARIABLE_NUMBER");

        if let Ok(limit) = env::var("SQLITE_MAX_EXPR_DEPTH") {
            cfg.flag(&format!("-DSQLITE_MAX_EXPR_DEPTH={}", limit));
        }
        println!("cargo:rerun-if-env-changed=SQLITE_MAX_EXPR_DEPTH");

        if let Ok(extras) = env::var("LIBSQLITE3_FLAGS") {
            for extra in extras.split_whitespace() {
                if extra.starts_with("-D") || extra.starts_with("-U") {
                    cfg.flag(extra);
                } else if extra.starts_with("SQLITE_") {
                    cfg.flag(&format!("-D{}", extra));
                } else {
                    panic!("Don't understand {} in LIBSQLITE3_FLAGS", extra);
                }
            }
        }
        println!("cargo:rerun-if-env-changed=LIBSQLITE3_FLAGS");

        cfg.compile("libsqlite3.a");

        println!("cargo:lib_dir={}", out_dir);
    }
}

fn env_prefix() -> &'static str {
    if cfg!(feature = "sqlcipher") {
        "SQLCIPHER"
    } else {
        "SQLITE3"
    }
}

fn header_file() -> &'static str {
    if cfg!(feature = "loadable_extension") {
        "sqlite3ext.h"
    } else {
        "sqlite3.h"
    }
}

fn wrapper_file() -> &'static str {
    if cfg!(feature = "loadable_extension") {
        "wrapper-ext.h"
    } else {
        "wrapper.h"
    }
}

pub enum HeaderLocation {
    FromEnvironment,
    Wrapper,
    FromPath(String),
}

impl From<HeaderLocation> for String {
    fn from(header: HeaderLocation) -> String {
        match header {
            HeaderLocation::FromEnvironment => {
                let prefix = env_prefix();
                let mut header = env::var(format!("{}_INCLUDE_DIR", prefix)).unwrap_or_else(|_| {
                    panic!(
                        "{}_INCLUDE_DIR must be set if {}_LIB_DIR is set",
                        prefix, prefix
                    )
                });
                header.push_str("/");
                header.push_str(header_file());
                header
            }
            HeaderLocation::Wrapper => wrapper_file().into(),
            HeaderLocation::FromPath(path) => path,
        }
    }
}

mod build_linked {
    use super::{bindings, env_prefix, header_file, HeaderLocation};
    use std::env;
    use std::path::Path;

    pub fn main(_out_dir: &str, out_path: &Path) {
        let header = find_sqlite();
        bindings::write_to_out_dir(header, out_path);
    }

    fn find_link_mode() -> &'static str {
        // If the user specifies SQLITE3_STATIC (or SQLCIPHER_STATIC), do static
        // linking, unless it's explicitly set to 0.
        match &env::var(format!("{}_STATIC", env_prefix())) {
            Ok(v) if v != "0" => "static",
            _ => "dylib",
        }
    }
    // Prints the necessary cargo link commands and returns the path to the header.
    fn find_sqlite() -> HeaderLocation {
        let link_lib = link_lib();

        println!("cargo:rerun-if-env-changed={}_INCLUDE_DIR", env_prefix());
        println!("cargo:rerun-if-env-changed={}_LIB_DIR", env_prefix());
        println!("cargo:rerun-if-env-changed={}_STATIC", env_prefix());
        if cfg!(all(feature = "vcpkg", target_env = "msvc")) {
            println!("cargo:rerun-if-env-changed=VCPKGRS_DYNAMIC");
        }

        // dependents can access `DEP_SQLITE3_LINK_TARGET` (`sqlite3` being the
        // `links=` value in our Cargo.toml) to get this value. This might be
        // useful if you need to ensure whatever crypto library sqlcipher relies
        // on is available, for example.
        println!("cargo:link-target={}", link_lib);

        // Allow users to specify where to find SQLite.
        if let Ok(dir) = env::var(format!("{}_LIB_DIR", env_prefix())) {
            // Try to use pkg-config to determine link commands
            let pkgconfig_path = Path::new(&dir).join("pkgconfig");
            env::set_var("PKG_CONFIG_PATH", pkgconfig_path);
            if pkg_config::Config::new().probe(link_lib).is_err() {
                // Otherwise just emit the bare minimum link commands.
                println!("cargo:rustc-link-lib={}={}", find_link_mode(), link_lib);
                println!("cargo:rustc-link-search={}", dir);
            }
            return HeaderLocation::FromEnvironment;
        }

        if let Some(header) = try_vcpkg() {
            return header;
        }

        // See if pkg-config can do everything for us.
        match pkg_config::Config::new()
            .print_system_libs(false)
            .probe(link_lib)
        {
            Ok(mut lib) => {
                if let Some(mut header) = lib.include_paths.pop() {
                    header.push(header_file());
                    HeaderLocation::FromPath(header.to_string_lossy().into())
                } else {
                    HeaderLocation::Wrapper
                }
            }
            Err(_) => {
                // No env var set and pkg-config couldn't help; just output the link-lib
                // request and hope that the library exists on the system paths. We used to
                // output /usr/lib explicitly, but that can introduce other linking problems;
                // see https://github.com/rusqlite/rusqlite/issues/207.
                println!("cargo:rustc-link-lib={}={}", find_link_mode(), link_lib);
                HeaderLocation::Wrapper
            }
        }
    }

    #[cfg(all(feature = "vcpkg", target_env = "msvc"))]
    fn try_vcpkg() -> Option<HeaderLocation> {
        // See if vcpkg can find it.
        if let Ok(mut lib) = vcpkg::Config::new().probe(link_lib()) {
            if let Some(mut header) = lib.include_paths.pop() {
                header.push(header_file());
                return Some(HeaderLocation::FromPath(header.to_string_lossy().into()));
            }
        }
        None
    }

    #[cfg(not(all(feature = "vcpkg", target_env = "msvc")))]
    fn try_vcpkg() -> Option<HeaderLocation> {
        None
    }

    fn link_lib() -> &'static str {
        if cfg!(feature = "sqlcipher") {
            "sqlcipher"
        } else {
            "sqlite3"
        }
    }
}

mod build_loadable_extension {
    use super::{bindings, env_prefix, header_file, HeaderLocation};
    use std::env;
    use std::path::Path;

    pub fn main(_out_dir: &str, out_path: &Path) {
        let header = find_sqlite();
        bindings::write_to_out_dir(header, out_path);
    }

    // Prints the necessary cargo link commands and returns the path to the header.
    fn find_sqlite() -> HeaderLocation {
        let link_lib = "sqlite3";
        println!("cargo:rerun-if-env-changed={}_INCLUDE_DIR", env_prefix());
        println!("cargo:rerun-if-env-changed={}_LIB_DIR", env_prefix());
        println!("cargo:rerun-if-env-changed={}_STATIC", env_prefix());
        if cfg!(all(feature = "vcpkg", target_env = "msvc")) {
            println!("cargo:rerun-if-env-changed=VCPKGRS_DYNAMIC");
        }
        // Allow users to specify where to find SQLite.
        if let Ok(dir) = env::var(format!("{}_LIB_DIR", env_prefix())) {
            // Try to use pkg-config to determine link commands
            let pkgconfig_path = Path::new(&dir).join("pkgconfig");
            env::set_var("PKG_CONFIG_PATH", pkgconfig_path);
            return HeaderLocation::FromEnvironment;
        }

        if let Some(header) = try_vcpkg() {
            return header;
        }

        // See if pkg-config can do everything for us.
        match pkg_config::Config::new()
            .print_system_libs(false)
            .probe(link_lib)
        {
            Ok(mut lib) => {
                if let Some(mut header) = lib.include_paths.pop() {
                    header.push(header_file());
                    HeaderLocation::FromPath(header.to_string_lossy().into())
                } else {
                    HeaderLocation::Wrapper
                }
            }
            Err(_) => HeaderLocation::Wrapper,
        }
    }

    #[cfg(all(feature = "vcpkg", target_env = "msvc"))]
    fn try_vcpkg() -> Option<HeaderLocation> {
        let link_lib = "sqlite3";
        // See if vcpkg can find it.
        if let Ok(mut lib) = vcpkg::Config::new().probe(link_lib) {
            if let Some(mut header) = lib.include_paths.pop() {
                header.push(header_file());
                return Some(HeaderLocation::FromPath(header.to_string_lossy().into()));
            }
        }
        None
    }

    #[cfg(not(all(feature = "vcpkg", target_env = "msvc")))]
    fn try_vcpkg() -> Option<HeaderLocation> {
        None
    }
}

#[cfg(not(feature = "buildtime_bindgen"))]
mod bindings {
    use super::HeaderLocation;

    use std::fs;
    use std::path::Path;

    static PREBUILT_BINDGEN_PATHS: &[&str] = &[
        "bindgen-bindings/bindgen_3.6.8",
        #[cfg(feature = "min_sqlite_version_3_7_16")]
        "bindgen-bindings/bindgen_3.7.16",
        #[cfg(any(
            feature = "bundled_bindings",
            feature = "bundled",
            all(windows, feature = "bundled-windows")
        ))]
        "sqlite3/bindgen_bundled_version",
    ];

    pub fn write_to_out_dir(_header: HeaderLocation, out_path: &Path) {
        let in_path = format!(
            "{}{}.rs",
            PREBUILT_BINDGEN_PATHS[PREBUILT_BINDGEN_PATHS.len() - 1],
            prebuilt_bindgen_ext()
        );
        fs::copy(in_path.to_owned(), out_path).unwrap_or_else(|_| {
            panic!(
                "Could not copy bindings to output directory from {}",
                in_path
            )
        });
    }

    fn prebuilt_bindgen_ext() -> &'static str {
        if cfg!(feature = "loadable_extension") {
            "-ext"
        } else {
            ""
        }
    }
}

#[cfg(feature = "buildtime_bindgen")]
mod bindings {
    use super::HeaderLocation;
    use bindgen::callbacks::{IntKind, ParseCallbacks};

    use std::fs::OpenOptions;
    use std::io::Write;
    use std::path::Path;

    #[derive(Debug)]
    struct SqliteTypeChooser;

    impl ParseCallbacks for SqliteTypeChooser {
        fn int_macro(&self, _name: &str, value: i64) -> Option<IntKind> {
            if value >= i32::min_value() as i64 && value <= i32::max_value() as i64 {
                Some(IntKind::I32)
            } else {
                None
            }
        }
    }

    // Are we generating the bundled bindings? Used to avoid emitting things
    // that would be problematic in bundled builds. This env var is set by
    // `upgrade.sh`.
    fn generating_bundled_bindings() -> bool {
        // Hacky way to know if we're generating the bundled bindings
        println!("cargo:rerun-if-env-changed=LIBSQLITE3_SYS_BUNDLING");
        match std::env::var("LIBSQLITE3_SYS_BUNDLING") {
            Ok(v) if v != "0" => true,
            _ => false,
        }
    }

    pub fn write_to_out_dir(header: HeaderLocation, out_path: &Path) {
        let header: String = header.into();
        let mut output = Vec::new();
        let mut bindings = bindgen::builder()
            .header(header.clone())
            .parse_callbacks(Box::new(SqliteTypeChooser))
            .rustfmt_bindings(true);

        if cfg!(feature = "unlock_notify") {
            bindings = bindings.clang_arg("-DSQLITE_ENABLE_UNLOCK_NOTIFY");
        }
        if cfg!(feature = "preupdate_hook") {
            bindings = bindings.clang_arg("-DSQLITE_ENABLE_PREUPDATE_HOOK");
        }
        if cfg!(feature = "session") {
            bindings = bindings.clang_arg("-DSQLITE_ENABLE_SESSION");
        }

        // When cross compiling unless effort is taken to fix the issue, bindgen
        // will find the wrong headers. There's only one header included by the
        // amalgamated `sqlite.h`: `stdarg.h`.
        //
        // Thankfully, there's almost no case where rust code needs to use
        // functions taking `va_list` (It's nearly impossible to get a `va_list`
        // in Rust unless you get passed it by C code for some reason).
        //
        // Arguably, we should never be including these, but we include them for
        // the cases where they aren't totally broken...
        let target_arch = std::env::var("TARGET").unwrap();
        let host_arch = std::env::var("HOST").unwrap();
        let is_cross_compiling = target_arch != host_arch;

        // Note that when generating the bundled file, we're essentially always
        // cross compiling.
        if generating_bundled_bindings() || is_cross_compiling {
            // Get rid of va_list, as it's not
            bindings = bindings
                .blacklist_function("sqlite3_vmprintf")
                .blacklist_function("sqlite3_vsnprintf")
                .blacklist_function("sqlite3_str_vappendf")
                .blacklist_type("va_list")
                .blacklist_type("__builtin_va_list")
                .blacklist_type("__gnuc_va_list")
                .blacklist_type("__va_list_tag")
                .blacklist_item("__GNUC_VA_LIST");
        }

        // rust-bindgen does not handle CPP macros that alias functions, so
        // when using sqlite3ext.h to support loadable extensions, the macros
        // that attempt to redefine sqlite3 API routines to be redirected through
        // the global sqlite3_api instance of the sqlite3_api_routines structure
        // do not result in any code production.
        //
        // Before defining wrappers to take their place, we need to blacklist
        // all sqlite3 API functions since none of their symbols will be
        // available directly when being loaded as an extension.
        #[cfg(feature = "loadable_extension")]
        {
            // some api functions do not have an implementation in sqlite3_api_routines
            // (for example: sqlite3_config, sqlite3_initialize, sqlite3_interrupt, ...).
            // while this isn't a problem for shared libraries (unless we actually try to
            // call them, it is better to blacklist them all so that the build will fail
            // if an attempt is made to call an extern function that we know won't exist
            // and to avoid undefined symbol issues when linking the loadable extension
            // rust code with other (e.g. non-rust) code
            bindings = bindings.blacklist_function(".*");
        }

        bindings
            .generate()
            .unwrap_or_else(|_| panic!("could not run bindgen on header {}", header))
            .write(Box::new(&mut output))
            .expect("could not write output of bindgen");
        let mut output = String::from_utf8(output).expect("bindgen output was not UTF-8?!");

        // Get the list of API functions supported by sqlite3_api_routines,
        // set the corresponding sqlite3 api routine to be blacklisted in the
        // final bindgen run, and add wrappers for each of the API functions to
        // dispatch the API call through a sqlite3_api global, which is defined
        // outside the generated bindings in lib.rs, either as a built-in static
        // or an extern symbol in the case of loadable_extension_embedded (i.e.
        // when the rust code will be a part of an extension but not implement
        // the extension entrypoint itself).
        #[cfg(feature = "loadable_extension")]
        {
            let api_routines_struct_name = "sqlite3_api_routines".to_owned();

            let api_routines_struct = match get_struct_by_name(&output, &api_routines_struct_name) {
                Some(s) => s,
                None => {
                    panic!(
                        "Failed to find struct {} in early bindgen output",
                        api_routines_struct_name
                    );
                }
            };

            #[cfg(feature = "loadable_extension")]
            {
                output.push_str(
                    r#"

// sqlite3_api is defined in lib.rs as either a static or an extern when compiled as a loadable_extension
#[cfg(feature = "loadable_extension")]
use crate::sqlite3_api;

"#,
                );
            }

            output.push_str(
                r"
// sqlite3 API wrappers to support loadable extensions (Note: these were generated from build.rs - not by rust-bindgen)

");

            // create wrapper for each field in api routines struct
            for field in &api_routines_struct.fields {
                let ident = match &field.ident {
                    Some(ident) => ident,
                    None => {
                        panic!("Unexpected anonymous field in sqlite");
                    }
                };
                let field_type = &field.ty;

                // construct global sqlite api function identifier from field identifier
                let api_fn_name = format!("sqlite3_{}", ident);

                // generate wrapper function and push it to output string
                let wrapper = generate_wrapper(ident, field_type, &api_fn_name);
                output.push_str(&wrapper);
            }

            output.push_str("\n");
        }

        let mut file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .open(out_path)
            .unwrap_or_else(|_| panic!("Could not write to {:?}", out_path));

        file.write_all(output.as_bytes())
            .unwrap_or_else(|_| panic!("Could not write to {:?}", out_path));
    }

    #[cfg(feature = "loadable_extension")]
    fn get_struct_by_name(bindgen_sources: &str, name: &str) -> Option<syn::ItemStruct> {
        let file = syn::parse_file(&bindgen_sources).expect("unable to parse early bindgen output");

        for item in &file.items {
            if let syn::Item::Struct(s) = item {
                if s.ident == name {
                    return Some(s.to_owned());
                }
            }
        }
        None
    }

    #[cfg(feature = "loadable_extension")]
    fn bare_fn_from_type_path(t: &syn::Type) -> syn::TypeBareFn {
        let path = match t {
            syn::Type::Path(tp) => &tp.path,
            _ => {
                panic!("type was not a type path");
            }
        };

        let mut path_args: Option<syn::PathArguments> = None;
        for segment in &path.segments {
            if segment.arguments.is_empty() {
                continue;
            }
            path_args = Some(segment.arguments.to_owned());
            break;
        }
        match path_args {
            Some(syn::PathArguments::AngleBracketed(p)) => {
                for gen_arg in p.args {
                    match gen_arg {
                        syn::GenericArgument::Type(syn::Type::BareFn(bf)) => {
                            return bf;
                        }
                        _ => {
                            panic!("parsed type was not a bare function as expected");
                        }
                    };
                }
            }
            _ => {
                panic!("parsed path args were not angle bracketed as expected");
            }
        };
        panic!("unexpected failure to parse bare function");
    }

    #[cfg(feature = "loadable_extension")]
    fn generate_wrapper(
        field_ident: &syn::Ident,
        syn_type: &syn::Type,
        api_fn_name: &str,
    ) -> String {
        use quote::quote;
        use syn::Token;

        let field_name = field_ident.to_string();
        let api_fn_ident = syn::Ident::new(&api_fn_name, field_ident.span());

        // add wrapper macro invocation to be appended to the generated bindings
        let bare_fn = bare_fn_from_type_path(syn_type);
        let api_fn_output = &bare_fn.output;

        // prepare inputs
        let mut api_fn_inputs = bare_fn.inputs.clone();

        // handle variadic api functions
        if bare_fn.variadic.is_some() {
            // until rust c_variadic support exists, we can't
            // transparently wrap variadic api functions.
            // generate specific set of args in place of
            // variadic for each function we care about.
            let var_arg_types: Vec<Option<syn::Type>> = match api_fn_name {
                "sqlite3_db_config" => {
                    let mut_int_type: syn::TypeReference = syn::parse2(quote!(&mut i32))
                        .expect("failed to parse mutable integer reference");
                    vec![None, Some(syn::Type::Reference(mut_int_type))]
                }
                _ => vec![None],
            };

            for (index, var_arg_type) in var_arg_types.iter().enumerate() {
                let mut input = api_fn_inputs[api_fn_inputs.len() - 1].clone();
                let input_ident =
                    syn::Ident::new(&format!("vararg{}", index + 1), field_ident.span());
                let colon = Token![:](field_ident.span());
                input.name = Some((input_ident, colon));
                if let Some(t) = var_arg_type.to_owned() {
                    input.ty = t;
                }
                api_fn_inputs.push(input);
            }
        }

        // get identifiers for each of the inputs to use in the api call
        let api_fn_input_idents: Vec<syn::Ident> = (&api_fn_inputs)
            .into_iter()
            .map(|input| match &input.name {
                Some((ident, _)) => ident.to_owned(),
                _ => {
                    panic!("Input has no name {:#?}", input);
                }
            })
            .collect();

        // generate wrapper and return it as a string
        let wrapper_tokens = quote! {
            pub unsafe fn #api_fn_ident(#api_fn_inputs) #api_fn_output {
                if sqlite3_api.is_null() {
                    panic!("sqlite3_api is null");
                }
                ((*sqlite3_api).#field_ident
                    .expect(stringify!("sqlite3_api contains null pointer for ", #field_name, " function")))(
                        #(#api_fn_input_idents),*
                )
            }
        };
        return format!("{}\n\n", wrapper_tokens.to_string());
    }
}
