# rthid-io

The filesystem side of rthid, used by the binary to keep App and dialogs IO free.

| Module | Purpose |
| --- | --- |
| `atomic` | File replacement through a temporary file beside the destination |
| `dir` | Visible directories and supported image files for the open dialog |
| `image` | Source decoding, shrinking and protection from overwriting the source |
| `paste` | Paths carried by terminal paste events |
| `path` | Home directory expansion for entered paths |
| `png` | PNG export and a background writer that writes the latest queued grid |

See the crate docs (`cargo doc -p rthid-io --open`) for the API.
