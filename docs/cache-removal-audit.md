# Auditoría de descuentos y scans (Fases 4–5)

Los resultados contienen únicamente rutas eliminadas con éxito, tamaños conocidos,
apps desinstaladas y comandos completados. `MainWindow::apply_removed` propaga ese
mismo resultado a ambos stores, al analizador y a Overview. Los fallos permanecen
visibles; Overview actualiza el resumen persistente mediante el escritor con debounce.

| Flujo | Resultado y descuento local |
| --- | --- |
| Cachés, desarrollo y sistema | `clean_workflow` → `CleanScanStore::apply_removed` (también usado por `apply_cleaned`); los comandos completados se descuentan por título. |
| Archivos grandes | `large_files_page` → resultado de Trash → ambos stores y analizador. |
| Apps y leftovers | `uninstaller_page` → `UninstallOutcome`; una desinstalación de paquete exitosa se publica incluso si falla el borrado posterior de datos. Los datos conservados pasan a leftovers. |
| Modelos y juegos | `review_items_page` → resultado de Trash; los targets fallidos permanecen en el inventario. |
| Papelera | `EmptyResult` identifica entradas borradas y bytes liberados; el popover conserva los fallos y Overview descuenta sin volver a enumerar. |
| Analizador | `StorageAnalysis::remove_paths`, con descuento adicional de tamaños de archivos ocultos por los límites del árbol; el resultado se propaga a los stores. |
| Reglas programadas y plan de limpieza | `dashboard_page` aplica `CleanResult` al mismo coordinador, también en resultados parcialmente bloqueados. El scan preparatorio de cada ejecución programada sigue existiendo; terminar de borrar no inicia otro. |

Los reportes y las actualizaciones de estado no llaman a scans ni a mediciones de
disco. Los requests forzados quedan en los controles explícitos de scan y el arranque.
Los resultados de scans anteriores al descuento no pueden reponer una instantánea
vieja. La lectura de disco de Overview también descarta un resultado anterior a un
descuento local.

`scan_jobs` comparte dos permisos entre los stores, Overview, analizador, scans de
reglas, previews de reglas y búsqueda de carpetas para reglas. Los permisos se
adquieren en trabajadores y se liberan con RAII; GTK nunca espera por un permiso.
Los helpers internos de un scan usan el permiso de su trabajador.

Las instantáneas sin cambios conservan su `Arc`; los cambios usan `Arc::make_mut`.
El filtrado de inventarios de limpieza se ejecuta fuera de GTK y comparte el delta
con `Arc`. Las páginas conservan sus filas cuando la instantánea no cambia y marcan
como pendientes los cambios recibidos mientras están ocultas. Overview compara
las lecturas antes de reconstruir sus tarjetas. Las miniaturas y portadas comparten
píxeles con `Arc` y se reutilizan por ruta, mtime y tamaño; los metadatos se leen en
el trabajador del scan.

## Mediciones reproducibles

`cargo test --offline performance -- --nocapture --test-threads=1` ejercita 50.000
entradas sin GTK ni acceso al disco. En este entorno, con perfil de pruebas sin
optimizar: `apply_cleaned` ≈94 ms (incluye despacho y filtrado en trabajador),
`forget_paths` ≈50 ms. Son muestras, no umbrales de aceptación.

Con `RUST_LOG=magnifico=debug cargo run --offline`, los eventos
`overview_values_visible` (ms desde el inicio hasta el primer frame con valores),
`forget_paths` y `apply_cleaned` (µs) permiten medir una sesión real. El arranque
visual requiere una pantalla GTK; las mediciones de inventario no la requieren.

Los tests cubren descuentos sin nuevos jobs ni loading, fallos parciales,
conservación de `Arc`, scans antiguos, reutilización e invalidación de medios,
descuentos del analizador y el máximo de dos scans activos.
