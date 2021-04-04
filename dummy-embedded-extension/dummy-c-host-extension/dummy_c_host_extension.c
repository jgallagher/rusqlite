// include the sqlite3 extension header and call macros as documented in https://sqlite.org/loadext.html
#include "sqlite3ext.h"
SQLITE_EXTENSION_INIT1

// include the cbindgen-generated bindings for the embedded extension
#include "dummy-embedded-extension.h"

// the extension entry point
int sqlite3_dummychostextension_init(
                                        sqlite3 *db,
                                        char **pzErrMsg,
                                        const sqlite3_api_routines *pApi
                                        ) {
  int rc = SQLITE_OK;
  SQLITE_EXTENSION_INIT2(pApi);

  // for this example, we essentially just pass through to the embedded
  // extension and return the result.
  rc = dummy_embedded_extension_init(db, pzErrMsg);

  return rc;
}
