fn main() {

	cpp_build::Config::new()
		.define("C4ENGINE", Some("1"))
		.include(r"C:\Users\tokgeo\source\repos\lc\src")
		.include(r"C:\Users\tokgeo\source\repos\lc\deps\include")
		.flag_if_supported("-std=c++20")
		.flag_if_supported("/std:c++20")
		//.cargo_metadata(true)
		.build("src/lib.rs");

	println!(r"cargo:rustc-link-search=C:\Users\tokgeo\source\repos\Detours\lib.X64");
	println!(r"cargo:rerun-if-changed=src/detour.rs");
	println!(r"cargo:rerun-if-changed=src/export.rs");
	println!(r"cargo:rerun-if-changed=src/http.rs");
	println!(r"cargo:rerun-if-changed=src/lib.rs");
	println!(r"cargo:rerun-if-changed=src/script.rs");
	println!(r"cargo:rerun-if-changed=src/window.rs");
}
