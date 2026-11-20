# Sidebar icons

Drop an SVG here named after the page and rebuild; it is embedded in the binary.
The UI requests these through `icon_image("sidebar/<name>")`.
Navigation uses theme icons directly.

| File                     | Page                         |
|--------------------------|------------------------------|
| `storage-analyzer.svg`   | Analizador de almacenamiento |
| `browsers.svg`           | Navegadores                  |
| `applications.svg`       | Aplicaciones                 |
| `custom-directories.svg` | Directorios personalizados   |
| `user-system.svg`        | Sistema de usuario           |

Use `currentColor` for strokes/fills (lines or filled shapes both work): it is
replaced by the sidebar text color, so icons follow light/dark themes and the
selected row. Any `viewBox` works; icons are drawn at 20×20 px.
