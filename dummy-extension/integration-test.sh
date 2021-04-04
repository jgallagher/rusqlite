#!/bin/bash

set -euf -o pipefail

# the crate dir is where this script is located
crate_dir="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"

# location of the cdylib extension within the target dir
dummy_extension="${crate_dir}/target/debug/libdummy_extension" # sqlite will try adding .so, .dll, .dylib to this on its own

# expected output from vtable query
expected_vtable_output="1"

# expected output from function query
expected_function_output="Dummy extension loaded correctly!"

>&2 echo "checking for sqlite3 shell"
sqlite3_cmd=$(which sqlite3)
>&2 echo "sqlite3 found: ${sqlite3_cmd}"

# build the dummy-extension crate
>&2 echo "building the dummy-extension crate in ${crate_dir}"
(cd "${crate_dir}" && cargo build --all-targets --verbose)
>&2 echo "successfully built the dummy-extension crate"

>&2 echo "running sqlite3 (${sqlite3_cmd}) to test loadable_extension ${dummy_extension} vtable"
actual_vtable_output=$(${sqlite3_cmd} -cmd ".load ${dummy_extension}" :memory: "SELECT value FROM dummy LIMIT 1;")
>&2 echo "sqlite3 command returned successfully from vtable test, checking output is as expected"
test "${actual_vtable_output}" = "${expected_vtable_output}" && echo "OK" || (echo "vtable output '${actual_vtable_output}' was not as expected '${expected_vtable_output}'"; echo "FAIL"; exit 1)

>&2 echo "running sqlite3 (${sqlite3_cmd}) to test loadable_extension ${dummy_extension} function"
actual_function_output=$(${sqlite3_cmd} -cmd ".load ${dummy_extension}" :memory: "SELECT dummy_test_function();")
>&2 echo "sqlite3 command returned successfully from function test, checking output is as expected"
test  "${actual_function_output}" = "${expected_function_output}" && echo "OK" || (echo "function output '${actual_function_output}' was not as expected '${expected_function_output}'"; echo "FAIL"; exit 1)

>&2 echo "All tests passed."
exit 0
