# Plan: caché persistente + refresco en segundo plano

Objetivo: al abrir, mostrar de inmediato los últimos valores; refrescar en segundo plano; al limpiar/borrar, descontar sin pantalla "Analizando tu disco…"; indicar junto a cada fila del sidebar cuál está refrescando.

Reglas generales
- Stale-while-revalidate: si hay datos (de caché o de un scan previo), NUNCA mostrar el loader a pantalla completa; solo indicador discreto.
- El loader a pantalla completa (`scan_loading.rs`) solo cuando no hay ningún dato.
- Ningún I/O de disco ni (de)serialización pesada en el hilo GTK.
- No romper tests existentes; `cargo test --offline` y `cargo clippy` limpios. Cadenas nuevas con `tr()`.

## Fase 1 — Caché persistente de resúmenes (`src/services/scan_cache.rs`, nuevo)
- Struct `ScanSummary` (serde, con `version: u32`): bytes por categoría (8: caches, dev, sistema, large, apps, leftovers, models, games), conteos (large, apps, leftovers, models, games), `trash_bytes`, `saved_at`.
- Archivo `Storage::data_dir()/cache/summary.json`. Escritura atómica (tmp + rename). Lectura tolerante: versión distinta o JSON corrupto => `None`.
- `load()` síncrono y barato al arrancar; `save()` desde hilo trabajador.
- Tests: ida y vuelta, corrupto, versión incorrecta.

## Fase 2 — Sidebar y Overview arrancan con la caché
- Al crear la ventana, leer el resumen y pintar de inmediato los tamaños del sidebar (en vez de "…"), y las tarjetas del Overview. Filas opcionales (AI Models / Games) visibles si el bytes cacheado > 0.
- Tras arrancar, lanzar `request_all(true)` de los stores en segundo plano (escalonado: no todos a la vez, para no saturar disco/CPU) sin loader a pantalla completa en Overview ni en las páginas.
- Guardar resumen: al terminar cada scan (hilo aparte, con debounce ~1 s) y en `close_request` (`src/application.rs:74`) de forma síncrona y rápida.

## Fase 3 — Indicador de refresco por fila del sidebar (`src/ui/main_window.rs`)
- Junto a la etiqueta de tamaño de cada fila, un `cleanup_fx::Spinner` pequeño (~12 px) visible solo mientras ese store/kind esté `loading`. Mapa: Browsers=CleanPageKind::AppCaches, UserSystem=DevTools, SystemCleanup=System, LargeFiles=Kind::LargeFiles, Applications=Kind::Apps, AiModels=Kind::Models, Games=Kind::Games. StorageAnalyzer también si analiza.
- Se actualiza suscribiéndose a `review_store.subscribe` y `clean_store.subscribe`. Sin saltos de ancho (reservar espacio; test de ancho del sidebar existente en main_window.rs:~1049 debe seguir pasando). Tooltip "Refreshing…".

## Fase 4 — Descuento sin "Analizando" al eliminar
- Auditar TODOS los flujos de borrado (limpieza de cachés/dev/sistema, large files, apps/uninstaller, AI models, games, papelera, analizador de almacenamiento, reglas programadas) y garantizar que solo llaman a las funciones de descuento local (`CleanScanStore::apply_cleaned`, `ReviewScanStore::forget_paths`, `StorageAnalysis::remove_paths`) y NO a `request(.., true)`/`request_all(true)`/`scan_everything`.
- Añadir lo que falte: descuento para Apps/leftovers en el store de review (ahora solo Large/Items), y que Overview + sidebar recalculen desde los stores (`render_cached`) sin poner `loading`.
- Tras descontar, guardar la caché (Fase 2).
- Test: simular borrado y comprobar que no se dispara ningún job de scan ni `loading=true`.

## Fase 5 — Rendimiento
- Medir (tiempo de arranque hasta valores visibles; tiempo de `forget_paths`/`apply_cleaned`).
- Evitar clones grandes innecesarios (usar `Arc::make_mut`/`Arc::try_unwrap` cuando sea único), no re-renderizar si el `Arc` no cambió ni los totales, limitar scans concurrentes (máx. 2 hilos de scan a la vez), no re-decodificar miniaturas/portadas ya existentes al refrescar (reusar `previews`/`covers` por ruta+mtime).

## Criterios de aceptación
- Segunda apertura: sidebar y Overview muestran los números anteriores en el primer frame.
- Spinner junto a cada fila mientras refresca; desaparece al terminar.
- Borrar algo: totales bajan al instante, sin pantalla de análisis, sin re-scan.
- `cargo build`, `cargo test --offline`, `cargo clippy --offline` pasan.
