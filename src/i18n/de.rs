//! German text. Technical terms users know from Docker stay in English: Container, Image,
//! Volume, Compose, Logs, Exec. Keys and `{placeholders}` must match the English source.
pub const ENTRIES: &[(&str, &str)] = &[
    ("Start", "Starten"),
    ("Stop", "Stoppen"),
    ("Restart", "Neu starten"),
    ("Delete", "Löschen"),
    (
        "Lifecycle actions support Docker and Podman API connections only",
        "Container-Aktionen unterstützen nur Docker- und Podman-API-Verbindungen",
    ),
    (
        "Runtime rows control the runtime itself; runtime control is unsupported for this endpoint",
        "Runtime-Zeilen steuern die Runtime selbst; für diesen Endpunkt wird das nicht unterstützt",
    ),
    (
        "Delete is disabled on runtime and grouping rows; select a container, Compose service, or Compose project",
        "Löschen ist für Runtime- und Gruppenzeilen deaktiviert; Container, Compose-Service oder Compose-Projekt auswählen",
    ),
    ("{project} / Services", "{project} / Services"),
    ("Standalone containers", "Einzelcontainer"),
    (
        "Select a container, Compose workload, Services group, or Standalone group; namespace and resource actions are disabled",
        "Container, Compose-Workload, Services- oder Einzelcontainer-Gruppe auswählen; Aktionen für Namespaces und Ressourcen sind deaktiviert",
    ),
    (
        "Runtime identity is unavailable; refresh before attempting lifecycle actions",
        "Runtime-Identität nicht verfügbar; vor Container-Aktionen aktualisieren",
    ),
    (
        "This target name cannot be safely confirmed",
        "Dieser Zielname lässt sich nicht sicher bestätigen",
    ),
    (
        "Kubernetes-managed or unidentified containers cannot be changed",
        "Von Kubernetes verwaltete oder nicht identifizierte Container können nicht geändert werden",
    ),
    (
        "Duplicate container IDs in inventory; refresh before changing anything",
        "Doppelte Container-IDs im Inventar; vor Änderungen aktualisieren",
    ),
    (
        "Stop all selected containers before deleting; force deletion is disabled",
        "Vor dem Löschen alle ausgewählten Container stoppen; erzwungenes Löschen ist deaktiviert",
    ),
    (
        "No eligible containers for {action} in this selection",
        "Keine passenden Container für {action} in dieser Auswahl",
    ),
    (
        "Container identity, ownership, or Compose labels changed; cancel and select it again",
        "Container-Identität, Eigentümer oder Compose-Labels haben sich geändert; abbrechen und neu auswählen",
    ),
    (
        "Container state changed; refresh and confirm a new operation",
        "Container-Zustand hat sich geändert; aktualisieren und den Vorgang neu bestätigen",
    ),
    ("Succeeded", "Erfolgreich"),
    ("Failed: {error}", "Fehlgeschlagen: {error}"),
    (
        "Outcome unknown: {error} · inspect state before retrying",
        "Ergebnis unbekannt: {error} · vor erneutem Versuch den Zustand prüfen",
    ),
    ("Not attempted: {reason}", "Nicht versucht: {reason}"),
    ("{a0}/{a1} completed", "{a0}/{a1} abgeschlossen"),
    (
        "finished · {a0} succeeded, {a1} failed, {a2} unknown, {a3} skipped",
        "fertig · {a0} erfolgreich, {a1} fehlgeschlagen, {a2} unbekannt, {a3} übersprungen",
    ),
    (
        "Lifecycle connection timed out",
        "Zeitüberschreitung der Verbindung bei der Container-Aktion",
    ),
    (
        "confirmed set failed preflight; no mutations sent",
        "bestätigte Auswahl hat die Vorprüfung nicht bestanden; nichts wurde geändert",
    ),
    (
        "previous outcome is unknown",
        "vorheriges Ergebnis ist unbekannt",
    ),
    ("Starting", "Wird gestartet"),
    ("Stopping", "Wird gestoppt"),
    ("Restarting", "Wird neu gestartet"),
    ("Deleting", "Wird gelöscht"),
    (
        "Compose project has no containers; returned to all runtimes. Retained resources are still available.",
        "Compose-Projekt hat keine Container; zurück zu allen Runtimes. Vorhandene Ressourcen bleiben verfügbar.",
    ),
    (
        "Opened the Compose project matching this directory. Backspace returns to all runtimes.",
        "Das zu diesem Verzeichnis passende Compose-Projekt wurde geöffnet. Backspace führt zurück zu allen Runtimes.",
    ),
    (
        "Multiple projects match this directory · Choose a connection",
        "Mehrere Projekte passen zu diesem Verzeichnis · Verbindung auswählen",
    ),
    (
        "Podman socket changed; refreshing inventory",
        "Podman-Socket hat sich geändert; Inventar wird aktualisiert",
    ),
    (
        "Podman machine is no longer listed; refresh after starting it separately",
        "Podman-Maschine ist nicht mehr aufgeführt; nach dem separaten Start aktualisieren",
    ),
    ("{error}", "{error}"),
    (
        "Demo mode: lifecycle mutations are disabled",
        "Demo-Modus: Container-Aktionen sind deaktiviert",
    ),
    (
        "Wait for the running Compose operation to finish first",
        "Zuerst das Ende des laufenden Compose-Vorgangs abwarten",
    ),
    (
        "Select a target in Performance before requesting an action",
        "Vor einer Aktion ein Ziel in Performance auswählen",
    ),
    (
        "Select a container or Compose workload",
        "Container oder Compose-Workload auswählen",
    ),
    (
        "Select the primary runtime, not an alias",
        "Primäre Runtime auswählen, keinen Alias",
    ),
    (
        "Runtime control unsupported for this endpoint; select a workload for container actions",
        "Runtime-Steuerung für diesen Endpunkt nicht unterstützt; für Container-Aktionen einen Workload auswählen",
    ),
    (
        "Runtime is offline; use Start to bring it online",
        "Runtime ist offline; mit Starten online bringen",
    ),
    (
        "Connection unavailable or aliased; select its connected primary context",
        "Verbindung nicht verfügbar oder Alias; den verbundenen primären Kontext auswählen",
    ),
    (
        "An action is already active on this runtime; other runtimes remain available. :action-results shows progress",
        "Auf dieser Runtime läuft bereits eine Aktion; andere Runtimes bleiben verfügbar. :action-results zeigt den Fortschritt",
    ),
    (
        "Selection changed; reopen Actions",
        "Auswahl hat sich geändert; Aktionen erneut öffnen",
    ),
    ("Actions · {a0}", "Aktionen · {a0}"),
    (
        "Demo mode: Compose operations are disabled",
        "Demo-Modus: Compose-Vorgänge sind deaktiviert",
    ),
    (
        "Another Compose operation is still running; :compose-output shows it",
        "Ein anderer Compose-Vorgang läuft noch; :compose-output zeigt ihn",
    ),
    (
        "Wait for running lifecycle actions to finish first",
        "Zuerst das Ende laufender Container-Aktionen abwarten",
    ),
    (
        "Select a Compose project or service",
        "Compose-Projekt oder Service auswählen",
    ),
    (
        "Select a connected runtime, not an offline engine or alias",
        "Verbundene Runtime auswählen, keine Offline-Engine und keinen Alias",
    ),
    (
        "Type the exact project name to confirm",
        "Zum Bestätigen den exakten Projektnamen eingeben",
    ),
    (
        "Another operation started; nothing was sent",
        "Ein anderer Vorgang wurde gestartet; nichts wurde gesendet",
    ),
    (
        "Install the security module first: tha module install security",
        "Zuerst das Sicherheitsmodul installieren: tha module install security",
    ),
    (
        "Select a container or single-container service",
        "Container oder Service mit einem Container auswählen",
    ),
    (
        "Showing the service's active queue; x cancels it",
        "Zeigt die aktive Warteschlange des Dienstes; x bricht sie ab",
    ),
    (
        "Select an image to inspect its security report",
        "Image auswählen, um seinen Sicherheitsbericht anzusehen",
    ),
    (
        "Scans run from the Security view (Ctrl+→ or Alt+4)",
        "Scans werden in der Sicherheitsansicht gestartet (Strg+→ oder Alt+4)",
    ),
    (
        "Demo mode: security scans and downloads are disabled",
        "Demo-Modus: Sicherheitsscans und Downloads sind deaktiviert",
    ),
    (
        "Install first: tha module install security (or installer MODULES=security)",
        "Zuerst installieren: tha module install security (oder Installer mit MODULES=security)",
    ),
    (
        "A security queue is already running",
        "Eine Sicherheits-Warteschlange läuft bereits",
    ),
    (
        "No checked images remain to scan; select images with m",
        "Keine markierten Images mehr zu scannen; Images mit m auswählen",
    ),
    (
        "Resize the terminal to review this confirmation before confirming",
        "Terminal vergrößern, um die Angaben vor dem Bestätigen zu prüfen",
    ),
    (
        "Name does not match; nothing was sent",
        "Name stimmt nicht überein; nichts wurde gesendet",
    ),
    ("Runtime no longer exists", "Runtime existiert nicht mehr"),
    (
        "Connection changed or is unavailable; select and confirm again",
        "Verbindung hat sich geändert oder ist nicht verfügbar; neu auswählen und bestätigen",
    ),
    (
        "Runtime provider changed; select and confirm again",
        "Runtime-Anbieter hat sich geändert; neu auswählen und bestätigen",
    ),
    (
        "Runtime is offline; use Start instead",
        "Runtime ist offline; stattdessen Starten verwenden",
    ),
    (
        "Refresh inventory before retrying",
        "Vor dem erneuten Versuch das Inventar aktualisieren",
    ),
    (
        "Runtime identity changed; select and confirm again",
        "Runtime-Identität hat sich geändert; neu auswählen und bestätigen",
    ),
    (
        "Container disappeared; select and confirm again",
        "Container ist verschwunden; neu auswählen und bestätigen",
    ),
    (
        "Operation IDs exhausted; restart Tha before another action",
        "Vorgangs-IDs aufgebraucht; Tha vor der nächsten Aktion neu starten",
    ),
    (
        " · {failed} failed, {unknown} unknown",
        " · {failed} fehlgeschlagen, {unknown} unbekannt",
    ),
    (
        "{a0} running{issues} · {summary} · :action-results",
        "{a0} läuft{issues} · {summary} · :action-results",
    ),
    (
        "{a0} runtimes finished{issues} · :action-results",
        "{a0} Runtimes fertig{issues} · :action-results",
    ),
    (
        "{a0} · {a1} · :action-results",
        "{a0} · {a1} · :action-results",
    ),
    ("command completed", "Befehl abgeschlossen"),
    ("{action} failed", "{action} fehlgeschlagen"),
    ("Outcome unknown", "Ergebnis unbekannt"),
    ("Not attempted", "Nicht versucht"),
    (
        "No lifecycle action results yet",
        "Noch keine Ergebnisse von Container-Aktionen",
    ),
    (
        "Action results · Choose a runtime",
        "Aktionsergebnisse · Runtime auswählen",
    ),
    (
        "No containers in the current Explorer scope; adjust workspace or search",
        "Keine Container im aktuellen Explorer-Bereich; Workspace oder Suche anpassen",
    ),
    ("Exec unavailable: {error}", "Exec nicht verfügbar: {error}"),
    (
        "Exec closed; the container was not stopped.",
        "Exec geschlossen; der Container wurde nicht gestoppt.",
    ),
    (
        "No retained Exec sessions. Select a running container and press e.",
        "Keine offenen Exec-Sitzungen. Laufenden Container auswählen und e drücken.",
    ),
    (
        "Exec sessions · {a0}/{a1} retained",
        "Exec-Sitzungen · {a0}/{a1} offen",
    ),
    (
        "{a0} [not in latest inventory]",
        "{a0} [nicht im aktuellen Inventar]",
    ),
    (
        "Tree search cleared; selected branch collapsed.",
        "Baumsuche gelöscht; ausgewählter Zweig eingeklappt.",
    ),
    (
        "Matching the current directory to Compose projects… (--overview skips this)",
        "Aktuelles Verzeichnis wird mit Compose-Projekten abgeglichen… (--overview überspringt das)",
    ),
    (
        "Offline · waiting for runtime discovery",
        "Offline · wartet auf Runtime-Erkennung",
    ),
    (
        "Exec is disabled in demo mode.",
        "Exec ist im Demo-Modus deaktiviert.",
    ),
    (
        "Select a container or service, then press e to exec.",
        "Container oder Service auswählen und für Exec e drücken.",
    ),
    (
        "Exec is unavailable on a disconnected or aliased engine.",
        "Exec ist auf einer getrennten Engine oder einem Alias nicht verfügbar.",
    ),
    (
        "Exec requires a running, non-Kubernetes-managed container.",
        "Exec benötigt einen laufenden Container, der nicht von Kubernetes verwaltet wird.",
    ),
    (
        "Exec · Choose a running container",
        "Exec · Laufenden Container auswählen",
    ),
    (
        "Selected Exec session #{id}; Enter resumes input if connected",
        "Exec-Sitzung #{id} ausgewählt; Enter setzt die Eingabe fort, falls verbunden",
    ),
    (
        "Context order: {a0} · o to sort again",
        "Kontext-Reihenfolge: {a0} · o sortiert erneut",
    ),
    (
        "Connection unavailable. Select a connected container and retry.",
        "Verbindung nicht verfügbar. Verbundenen Container auswählen und erneut versuchen.",
    ),
    (
        "Sort contexts · pins stay first",
        "Kontexte sortieren · Fixierte bleiben oben",
    ),
    ("Namespace: {namespace}", "Namespace: {namespace}"),
    (" [alias]", " [Alias]"),
    (" (offline)", " (offline)"),
    (" [connecting]", " [verbindet]"),
    ("Services ({a0})", "Services ({a0})"),
    ("Standalone ({a0})", "Einzelcontainer ({a0})"),
    ("Networks", "Netzwerke"),
    ("Volumes", "Volumes"),
    (" [shared/external]", " [geteilt/extern]"),
    ("Shared ", "Geteilte "),
    ("{name} [stopped]", "{name} [gestoppt]"),
    (
        "{name} [{running}/{total} running]",
        "{name} [{running}/{total} läuft]",
    ),
    (
        "Compose update (pull, then up -d)",
        "Compose-Update (pull, dann up -d)",
    ),
    ("Compose pull", "Compose pull"),
    ("Compose up -d", "Compose up -d"),
    (
        "Compose up -d --force-recreate",
        "Compose up -d --force-recreate",
    ),
    (
        "Compose down (keeps volumes)",
        "Compose down (behält Volumes)",
    ),
    (
        "Downloads newer images, then recreates containers whose image or configuration changed.",
        "Ruft neuere Images ab und erstellt dann Container neu, deren Image oder Konfiguration sich geändert hat.",
    ),
    (
        "Downloads newer images. Running containers are not changed.",
        "Ruft neuere Images ab. Laufende Container bleiben unverändert.",
    ),
    (
        "Creates or recreates containers whose configuration changed.",
        "Erstellt Container oder erstellt jene neu, deren Konfiguration sich geändert hat.",
    ),
    (
        "Recreates every container, even if nothing changed.",
        "Erstellt jeden Container neu, auch wenn sich nichts geändert hat.",
    ),
    (
        "Stops and removes the project's containers and networks. Volumes and images are kept.",
        "Stoppt und entfernt Container und Netzwerke des Projekts. Volumes und Images bleiben erhalten.",
    ),
    (
        "This Compose name cannot be safely used on a command line",
        "Dieser Compose-Name lässt sich nicht sicher auf der Kommandozeile verwenden",
    ),
    (
        "No containers of this Compose project are available",
        "Keine Container dieses Compose-Projekts verfügbar",
    ),
    (
        "This project's containers came from different Compose configurations",
        "Die Container dieses Projekts stammen aus unterschiedlichen Compose-Konfigurationen",
    ),
    (
        "Kubernetes-managed containers cannot be changed",
        "Von Kubernetes verwaltete Container können nicht geändert werden",
    ),
    (
        "These containers have no Compose working directory label",
        "Diese Container haben kein Compose-Label für das Arbeitsverzeichnis",
    ),
    (
        "These containers have no Compose file label",
        "Diese Container haben kein Compose-Label für die Datei",
    ),
    (
        "Compose labels contain an unsafe path; nothing will run",
        "Compose-Labels enthalten einen unsicheren Pfad; es wird nichts ausgeführt",
    ),
    (
        "Compose actions support Docker and Podman API connections only",
        "Compose-Aktionen unterstützen nur Docker- und Podman-API-Verbindungen",
    ),
    (
        "Compose down applies to a whole project; select the project row",
        "Compose down gilt für das ganze Projekt; die Projektzeile auswählen",
    ),
    (
        "Install the Docker Compose plugin (docker compose) or docker-compose to use this",
        "Dafür das Docker-Compose-Plugin (docker compose) oder docker-compose installieren",
    ),
    (
        "Cannot start Compose",
        "Compose kann nicht gestartet werden",
    ),
    ("Connected first", "Verbundene zuerst"),
    ("Name", "Name"),
    ("CPU usage (snapshot)", "CPU-Auslastung (Momentaufnahme)"),
    (
        "Memory usage (snapshot)",
        "Speichernutzung (Momentaufnahme)",
    ),
    (
        "Exec requires a running, non-Kubernetes-managed container with an ID",
        "Exec benötigt einen laufenden Container mit ID, der nicht von Kubernetes verwaltet wird",
    ),
    (
        "exec_command must contain an executable",
        "exec_command muss ein ausführbares Programm enthalten",
    ),
    (
        "Exec currently requires a local Unix socket",
        "Exec benötigt derzeit einen lokalen Unix-Socket",
    ),
    (
        "Install nerdctl and add it to PATH to use exec",
        "Für Exec nerdctl installieren und zum PATH hinzufügen",
    ),
    (
        "Install the Docker or Podman CLI and add it to PATH to use exec; API browsing does not require a CLI",
        "Für Exec die Docker- oder Podman-CLI installieren und zum PATH hinzufügen; zum Ansehen über die API ist keine CLI nötig",
    ),
    ("up to date with the registry", "aktuell laut Registry"),
    (
        "registry digest differs for {a0} · :pull-image",
        "Registry-Digest weicht ab für {a0} · :pull-image",
    ),
    ("unknown · {reason}", "unbekannt · {reason}"),
    (
        "no registry digest locally; it may be a local build",
        "kein lokaler Registry-Digest; möglicherweise ein lokaler Build",
    ),
    (
        "the registry needs a login, or the image is private",
        "die Registry verlangt eine Anmeldung, oder das Image ist privat",
    ),
    (
        "not found in a registry, or the engine lacks this API",
        "nicht in einer Registry gefunden, oder der Engine fehlt diese API",
    ),
    (
        "the registry reported no digest",
        "die Registry hat keinen Digest gemeldet",
    ),
    (
        "the registry did not answer in time",
        "die Registry hat nicht rechtzeitig geantwortet",
    ),
    (
        "only digest references, which do not move",
        "nur Digest-Referenzen, die sich nicht ändern",
    ),
    (
        "Checked {total} images: {available} with updates, {unknown} unknown. Enter shows details.",
        "Geprüfte Images: {total}, davon {available} mit Updates, {unknown} unbekannt. Enter zeigt Details.",
    ),
    (
        "Install the Docker or Podman CLI and add it to PATH to pull images",
        "Zum Abrufen von Images die Docker- oder Podman-CLI installieren und zum PATH hinzufügen",
    ),
    (
        "Cannot start {program}",
        "{program} kann nicht gestartet werden",
    ),
    ("Pulling {tag}: {line}", "{tag} wird abgerufen: {line}"),
    (
        "{program} pull exited with {status}",
        "{program} pull endete mit {status}",
    ),
    (
        "Pulled {a0}. Recreate containers to use the new image.",
        "{a0} abgerufen. Container neu erstellen, um das neue Image zu verwenden.",
    ),
    (
        "Pull failed for {a0}: {error}",
        "Abrufen fehlgeschlagen für {a0}: {error}",
    ),
    (
        "Only the first {MAX_ROWS} rows in each section are retained",
        "Nur die ersten {MAX_ROWS} Zeilen je Abschnitt werden behalten",
    ),
    (
        "Image layers lookup timed out after 6s",
        "Zeitüberschreitung bei der Abfrage der Image-Layer nach 6 s",
    ),
    (
        "Use a duration such as 15m, 2h, 30s or 1d, an RFC 3339 time, or off",
        "Dauer wie 15m, 2h, 30s oder 1d angeben, eine RFC-3339-Zeit oder off",
    ),
    (
        "The log range must start in the past",
        "Der Zeitraum der Logs muss in der Vergangenheit beginnen",
    ),
    (
        "That time is outside the range the Docker API accepts",
        "Diese Zeit liegt außerhalb des von der Docker-API akzeptierten Bereichs",
    ),
    ("Log stream reconnected.", "Log-Stream wieder verbunden."),
    (
        "Showing the first {MAX_STREAMS} containers. Select a service for a smaller scope.",
        "Die ersten {MAX_STREAMS} Container werden angezeigt. Für einen kleineren Bereich einen Service auswählen.",
    ),
    (
        "Earlier lines were dropped to stay within the {a0} MiB per-container history limit.",
        "Ältere Zeilen wurden verworfen, um das Limit von {a0} MiB Verlauf je Container einzuhalten.",
    ),
    (
        "Container no longer exists; stream closed.",
        "Container existiert nicht mehr; Stream geschlossen.",
    ),
    (
        "Logs unavailable: {error}. Tha keeps retrying automatically.",
        "Logs nicht verfügbar: {error}. Tha versucht es automatisch weiter.",
    ),
    (
        "Stream ended (container stopped or restarting). Tha resumes automatically when new output appears.",
        "Stream beendet (Container gestoppt oder im Neustart). Tha macht automatisch weiter, sobald neue Ausgabe erscheint.",
    ),
    (
        "Choose an explicit destination filename",
        "Einen expliziten Zieldateinamen wählen",
    ),
    (
        "Export exceeds the 16 MiB snapshot limit; narrow the filter first. No file created",
        "Export überschreitet das Snapshot-Limit von 16 MiB; zuerst den Filter einschränken. Keine Datei erstellt",
    ),
    (
        "No retained entries match this filter; no file created",
        "Keine gespeicherten Einträge passen zu diesem Filter; keine Datei erstellt",
    ),
    (
        "Cannot create export: destination must be a new file in an existing directory",
        "Export nicht möglich: Ziel muss eine neue Datei in einem vorhandenen Verzeichnis sein",
    ),
    (
        "Export write failed; a partial new export file may remain",
        "Schreiben des Exports fehlgeschlagen; eine unvollständige neue Exportdatei kann zurückbleiben",
    ),
    (
        "Exported {a0} retained entries to {a1} (JSONL, mode 0600; may contain secrets)",
        "{a0} gespeicherte Einträge nach {a1} exportiert (JSONL, Modus 0600; kann Geheimnisse enthalten)",
    ),
    (
        "Paste contains control characters or exceeds 1024 bytes; nothing pasted",
        "Einfügen enthält Steuerzeichen oder überschreitet 1024 Bytes; nichts eingefügt",
    ),
    ("Finished: {a0}", "Fertig: {a0}"),
    (
        "Failed: {a0} · {error} · :compose-output shows details",
        "Fehlgeschlagen: {a0} · {error} · :compose-output zeigt Details",
    ),
    (
        "Security scan failed for {a0}: {a1}",
        "Sicherheitsscan fehlgeschlagen für {a0}: {a1}",
    ),
    (
        "Connected to daemon metrics; local stats polling stopped",
        "Mit Daemon-Metriken verbunden; lokale Stats-Abfrage gestoppt",
    ),
    (
        "Daemon stopped; local stats sampling resumed",
        "Daemon gestoppt; lokale Stats-Erfassung wieder aufgenommen",
    ),
    (
        "Cannot check daemon: {error}",
        "Daemon-Prüfung nicht möglich: {error}",
    ),
    (
        "Exec #{a0} {a1} · container was not stopped. F6 switches retained sessions.",
        "Exec #{a0} {a1} · Container wurde nicht gestoppt. F6 wechselt zwischen offenen Sitzungen.",
    ),
    (
        "Log export failed: {error}",
        "Log-Export fehlgeschlagen: {error}",
    ),
    (
        "Log export worker failed: {error}",
        "Log-Export-Worker fehlgeschlagen: {error}",
    ),
    (
        "Security queue cancellation requested",
        "Abbruch der Sicherheits-Warteschlange angefordert",
    ),
    (
        "Cannot cancel security queue: {error}",
        "Sicherheits-Warteschlange kann nicht abgebrochen werden: {error}",
    ),
    (
        "Security queue is still running or cleaning up",
        "Sicherheits-Warteschlange läuft noch oder räumt auf",
    ),
    (
        "Connection unavailable or aliased. Browse a connected engine to view logs.",
        "Verbindung nicht verfügbar oder Alias. Für Logs eine verbundene Engine öffnen.",
    ),
    (
        "No containers in this scope.",
        "Keine Container in diesem Bereich.",
    ),
    (
        "Review the export dialog in a terminal at least 60 × 18 first",
        "Den Export-Dialog zuerst in einem Terminal mit mindestens 60 × 18 prüfen",
    ),
    (
        "Another export is already running",
        "Ein anderer Export läuft bereits",
    ),
    (
        "Log selection disappeared",
        "Log-Auswahl ist nicht mehr vorhanden",
    ),
    (
        "Log target changed or is unavailable; cancel and reopen export after reviewing Logs",
        "Log-Ziel hat sich geändert oder ist nicht verfügbar; abbrechen und Export nach Prüfung der Logs neu öffnen",
    ),
    (
        "Exec is still connected · navigation mode · e refocuses the same container, or select 4 then Enter.",
        "Exec ist weiter verbunden · Navigationsmodus · e öffnet denselben Container wieder, oder 4 und dann Enter.",
    ),
    (
        "Deleting {a0} resources · Esc hides progress; no automatic retries",
        "{a0} Ressourcen werden gelöscht · Esc blendet Fortschritt aus; keine automatischen Wiederholungen",
    ),
    (
        "Runtime changed; deletion cancelled",
        "Runtime hat sich geändert; Löschen abgebrochen",
    ),
    (
        "Applied retained-log filter; entries without required metadata do not match. :clear-log-filter resets it.",
        "Log-Filter angewendet; Einträge ohne die nötigen Metadaten passen nicht. :clear-log-filter setzt ihn zurück.",
    ),
    (
        "Invalid log filter: {error}",
        "Ungültiger Log-Filter: {error}",
    ),
    ("Invalid regex: {error}", "Ungültiger Regex: {error}"),
    ("Invalid regex", "Ungültiger Regex"),
    (
        "{a0} is unavailable in {a1}",
        "{a0} ist nicht verfügbar in {a1}",
    ),
    (
        "Exec requires a running container or service.",
        "Exec benötigt einen laufenden Container oder Service.",
    ),
    (
        "Demo mode: fixture logs ignore the start time.",
        "Demo-Modus: Beispiel-Logs ignorieren die Startzeit.",
    ),
    (
        "No Compose operation has run in this session",
        "In dieser Sitzung wurde noch kein Compose-Vorgang ausgeführt",
    ),
    (
        "Cancelling security queue and removing scanner…",
        "Sicherheits-Warteschlange wird abgebrochen und Scanner entfernt…",
    ),
    (
        "No image/volume cleanup results yet",
        "Noch keine Ergebnisse der Image-/Volume-Bereinigung",
    ),
    (
        "Demo mode: fixture data is unchanged.",
        "Demo-Modus: Beispieldaten bleiben unverändert.",
    ),
    (
        "Refreshing all engines…",
        "Alle Engines werden aktualisiert…",
    ),
    (
        "Layers requires a container or single-container service",
        "Layer benötigt einen Container oder Service mit einem Container",
    ),
    (
        "Select a container to open Metrics",
        "Container auswählen, um Metriken zu öffnen",
    ),
    (
        "Open and review Logs for the desired workload before exporting",
        "Vor dem Export Logs des gewünschten Workloads öffnen und prüfen",
    ),
    (
        "Unknown command: {command}. Press : then Tab to discover commands.",
        "Unbekannter Befehl: {command}. : und dann Tab zeigt verfügbare Befehle.",
    ),
    (
        "Standalone session · last 60 sampling batches only",
        "Standalone-Sitzung · nur die letzten 60 Messungen",
    ),
    ("stopped", "gestoppt"),
    ("stale", "veraltet"),
    ("warm", "misst"),
    (
        "CPU {a0}/{a1} · memory {a2}/{a3} live running samples{a4}{a5}",
        "CPU {a0}/{a1} · Speicher {a2}/{a3} aktuelle Messwerte laufender Container{a4}{a5}",
    ),
    (" · {a0} CPU warming up", " · {a0} CPU wird noch gemessen"),
    (" · {a0} stale", " · {a0} veraltet"),
    (
        "{destination} — exposed, no host port",
        "{destination} — freigegeben, kein Host-Port",
    ),
    (
        "host port {port} → {destination} (host address not reported)",
        "Host-Port {port} → {destination} (Host-Adresse nicht gemeldet)",
    ),
    (
        "{local} → {destination} (loopback only · {ip})",
        "{local} → {destination} (nur Loopback · {ip})",
    ),
    (
        "{host} → {destination} (all interfaces; not localhost-only)",
        "{host} → {destination} (alle Schnittstellen; nicht nur localhost)",
    ),
    (
        "{host} → {destination} (specific host address)",
        "{host} → {destination} (bestimmte Host-Adresse)",
    ),
    ("Invalid Colima profile", "Ungültiges Colima-Profil"),
    ("Colima profile {profile}", "Colima-Profil {profile}"),
    ("Podman machine {machine}", "Podman-Maschine {machine}"),
    ("Docker Desktop", "Docker Desktop"),
    (
        "OrbStack (Docker and all Linux machines)",
        "OrbStack (Docker und alle Linux-Maschinen)",
    ),
    (
        "Runtime deletion is disabled",
        "Löschen von Runtimes ist deaktiviert",
    ),
    (
        "Runtime socket mapping changed or is ambiguous; refresh and select the correct provider",
        "Runtime-Socket-Zuordnung hat sich geändert oder ist mehrdeutig; aktualisieren und den richtigen Anbieter auswählen",
    ),
    (
        "Colima profile no longer exists; no VM was created",
        "Colima-Profil existiert nicht mehr; es wurde keine VM erstellt",
    ),
    (
        "Cannot run {program}; install its CLI and ensure it is on PATH",
        "{program} kann nicht ausgeführt werden; die CLI installieren und sicherstellen, dass sie im PATH liegt",
    ),
    (
        "{total} image(s) queued; continues after image errors",
        "Images in der Warteschlange: {total}; läuft bei Image-Fehlern weiter",
    ),
    (
        "Starting security queue…",
        "Sicherheits-Warteschlange wird gestartet…",
    ),
    (
        "Cannot load saved report: {error}",
        "Gespeicherter Bericht kann nicht geladen werden: {error}",
    ),
    ("Queue cancelled", "Warteschlange abgebrochen"),
    ("Queue complete", "Warteschlange abgeschlossen"),
    (
        "{status} · {completed} completed · {failed} failed · completed reports saved to SQLite",
        "{status} · {completed} abgeschlossen · {failed} fehlgeschlagen · fertige Berichte in SQLite gespeichert",
    ),
    (
        "Image scan failed: {error}",
        "Image-Scan fehlgeschlagen: {error}",
    ),
    (
        "Exec session limit must be 1–8; close retained sessions before reducing it",
        "Exec-Sitzungslimit muss 1–8 sein; vor dem Verringern offene Sitzungen schließen",
    ),
    (
        "Exec session limit reached ({open}/{limit} retained). Use F6 to select one and :close-exec to free a slot",
        "Exec-Sitzungslimit erreicht ({open}/{limit} offen). Mit F6 eine auswählen und mit :close-exec einen Platz freigeben",
    ),
    (
        "Exec session IDs exhausted",
        "Exec-Sitzungs-IDs aufgebraucht",
    ),
    (
        "Security module is disabled",
        "Sicherheitsmodul ist deaktiviert",
    ),
    ("Select a workload", "Workload auswählen"),
    ("Select a container", "Container auswählen"),
    (
        "Scans run from the Security view",
        "Scans werden in der Sicherheitsansicht gestartet",
    ),
    (
        "Select a running container or service",
        "Laufenden Container oder Service auswählen",
    ),
    (
        "Scans are disabled in demo mode",
        "Scans sind im Demo-Modus deaktiviert",
    ),
    (
        "No active security queue",
        "Keine aktive Sicherheits-Warteschlange",
    ),
    (
        "Open and review Logs before exporting",
        "Vor dem Export Logs öffnen und prüfen",
    ),
    ("Another export is running", "Ein anderer Export läuft"),
    (
        "No cleanup results yet",
        "Noch keine Bereinigungsergebnisse",
    ),
    ("No lifecycle results yet", "Noch keine Aktionsergebnisse"),
    ("Runtime: {a0}", "Runtime: {a0}"),
    ("Directory: {a0}", "Verzeichnis: {a0}"),
    ("File: {a0}", "Datei: {a0}"),
    ("Env file: {a0}", "Env-Datei: {a0}"),
    (
        "These paths come from container labels and Compose runs what they contain. Check them.",
        "Diese Pfade stammen aus Container-Labels, und Compose führt ihren Inhalt aus. Bitte prüfen.",
    ),
    (
        "Type {a0} to confirm: {a1}▏",
        "Zum Bestätigen {a0} eingeben: {a1}▏",
    ),
    (
        " Enter Confirm · Esc Cancel ",
        " Enter Bestätigen · Esc Abbrechen ",
    ),
    ("Run this now?", "Jetzt ausführen?"),
    (
        " y/Enter Run · n/Esc Cancel ",
        " y/Enter Ausführen · n/Esc Abbrechen ",
    ),
    (" Compose ", " Compose "),
    ("Running…", "Läuft…"),
    ("Finished", "Fertig"),
    (
        " j/k Scroll · G Latest · Esc Close; a running operation continues ",
        " j/k Scrollen · G Neueste · Esc Schließen; ein laufender Vorgang geht weiter ",
    ),
    (
        "{total} events · newest first · UTC · {a0}",
        "Ereignisse: {total} · neueste zuerst · UTC · {a0}",
    ),
    (
        "last 24 hours, saved by the service",
        "letzte 24 Stunden, vom Dienst gespeichert",
    ),
    (
        "from the engine while Tha is open",
        "von der Engine, solange Tha offen ist",
    ),
    (
        "Events are not available for native containerd connections.",
        "Ereignisse sind für native containerd-Verbindungen nicht verfügbar.",
    ),
    (
        "No events for this selection yet. Starts, exits, OOM kills and health changes appear here as they happen.",
        "Noch keine Ereignisse für diese Auswahl. Starts, Beendigungen, OOM-Kills und Health-Änderungen erscheinen hier, sobald sie passieren.",
    ),
    ("unavailable", "nicht verfügbar"),
    ("KiB", "KiB"),
    ("MiB", "MiB"),
    ("GiB", "GiB"),
    ("TiB", "TiB"),
    ("PiB", "PiB"),
    ("EiB", "EiB"),
    (
        "Select a container to analyze its metrics",
        "Container auswählen, um seine Metriken zu analysieren",
    ),
    (" Metrics · {a0} ", " Metriken · {a0} "),
    ("Range: ", "Zeitraum: "),
    (
        "· {a0} shorter / {a1} longer",
        "· {a0} kürzer / {a1} länger",
    ),
    (
        "Latest counters ({a0}s old): Disk R/W {a1} / {a2} · Net Rx/Tx {a3} / {a4}",
        "Letzte Zähler (vor {a0}s): Disk R/W {a1} / {a2} · Net Rx/Tx {a3} / {a4}",
    ),
    (
        "Latest I/O counters unavailable",
        "Letzte I/O-Zähler nicht verfügbar",
    ),
    (
        "{a0} · {a1} → {a2} UTC · {a3}{a4}",
        "{a0} · {a1} → {a2} UTC · {a3}{a4}",
    ),
    ("Paused", "Pausiert"),
    ("Live", "Live"),
    (" · Loading…", " · Lädt…"),
    (
        "┊ {a0} events in range · latest {a1} {a2} · 5 Events",
        "┊ Ereignisse im Zeitraum: {a0} · zuletzt {a1} {a2} · 5 Ereignisse",
    ),
    (
        "←/→ Pan · c Cursor · h/l Inspect · f Live · z Enlarge",
        "←/→ Verschieben · c Cursor · h/l Ansehen · f Live · z Vergrößern",
    ),
    (
        "j/k Metric (small terminal) · Esc Restore · gaps = missing · 100% CPU = 1 core · auto scale",
        "j/k Metrik (kleines Terminal) · Esc Wiederherstellen · Lücken = fehlende Werte · 100% CPU = 1 Kern · Skala automatisch",
    ),
    (
        "No readings in this time range. Collection begins when Tha/daemon observes this container. {shorter} selects a shorter range; f returns to live.",
        "Keine Messwerte in diesem Zeitraum. Die Erfassung beginnt, sobald Tha oder der Daemon diesen Container beobachtet. {shorter} wählt einen kürzeren Zeitraum; f kehrt zu Live zurück.",
    ),
    ("Memory", "Speicher"),
    ("Disk read / write", "Disk Lesen / Schreiben"),
    ("Network receive / transmit", "Netzwerk Empfang / Senden"),
    ("avg {a0} / peak {a1}", "Ø {a0} / Spitze {a1}"),
    (" · {cores} cores", " · {cores} Kerne"),
    (" {a0} · {summary} · auto ", " {a0} · {summary} · auto "),
    (
        "read blue / write magenta",
        "Lesen blau / Schreiben magenta",
    ),
    ("Rx blue / Tx yellow", "Rx blau / Tx gelb"),
    ("dotted = average", "gepunktet = Durchschnitt"),
    ("latest", "aktuell"),
    ("Tags: {a0}", "Tags: {a0}"),
    (
        "Host path → container destination; not a managed volume",
        "Host-Pfad → Ziel im Container; kein verwaltetes Volume",
    ),
    (
        "Open Images or Volumes to prune unreferenced resources",
        "Images oder Volumes öffnen, um ungenutzte Ressourcen zu bereinigen",
    ),
    (
        "Resource cleanup is already awaiting confirmation or running",
        "Eine Ressourcen-Bereinigung wartet bereits auf Bestätigung oder läuft",
    ),
    (
        "Demo mode: resource deletion is disabled",
        "Demo-Modus: Löschen von Ressourcen ist deaktiviert",
    ),
    (
        "Bind mounts cannot be deleted from Tha",
        "Bind-Mounts können nicht aus Tha gelöscht werden",
    ),
    (
        "Select a connected primary runtime",
        "Verbundene primäre Runtime auswählen",
    ),
    (
        "Resource removal is unsupported for this backend",
        "Entfernen von Ressourcen wird für dieses Backend nicht unterstützt",
    ),
    (
        "No unreferenced resources to prune; running/stopped references and unknown usage are protected",
        "Keine ungenutzten Ressourcen zum Bereinigen; laufende/gestoppte Referenzen und unbekannte Nutzung sind geschützt",
    ),
    (
        "Select unreferenced resources first; running/stopped references and unknown usage are protected",
        "Zuerst ungenutzte Ressourcen auswählen; laufende/gestoppte Referenzen und unbekannte Nutzung sind geschützt",
    ),
    (
        "Runtime identity unavailable; deletion refused",
        "Runtime-Identität nicht verfügbar; Löschen verweigert",
    ),
    (
        " Confirm resource deletion ",
        " Löschen von Ressourcen bestätigen ",
    ),
    (
        "Resize to at least 70 × 19 to review deletion. Esc cancels.",
        "Auf mindestens 70 × 19 vergrößern, um das Löschen zu prüfen. Esc bricht ab.",
    ),
    (
        "Runtime: {a0}\nRemove {a1} {a2}?\n{a3}\nRunning AND stopped references rechecked before EACH deletion.\nNo force deletion or parent-image pruning.",
        "Runtime: {a0}\n{a1} {a2} entfernen?\n{a3}\nLaufende UND gestoppte Referenzen werden vor JEDEM Löschen erneut geprüft.\nKein erzwungenes Löschen und kein Entfernen übergeordneter Images.",
    ),
    ("managed volumes", "verwaltete Volumes"),
    (
        "WARNING: this permanently destroys all data in the listed volumes.",
        "WARNUNG: Alle Daten in den aufgeführten Volumes werden unwiderruflich gelöscht.",
    ),
    (
        "Image sizes include shared layers; they are NOT reclaimable-space estimates.",
        "Image-Größen enthalten geteilte Layer; sie zeigen NICHT den freiwerdenden Speicher.",
    ),
    (
        "Resize wider/taller to review the complete deletion confirmation. Esc cancels.",
        "Terminal vergrößern, um die vollständige Löschbestätigung zu prüfen. Esc bricht ab.",
    ),
    (
        "Cleanup running · Esc Hide (does not undo or cancel sent requests)\n↑/↓ or PgUp/PgDn Review names",
        "Bereinigung läuft · Esc Ausblenden (macht gesendete Anfragen nicht rückgängig)\n↑/↓ oder PgUp/PgDn Namen prüfen",
    ),
    (
        "y / Enter Confirm · n / Esc Cancel\n↑/↓ or PgUp/PgDn Review names",
        "y / Enter Bestätigen · n / Esc Abbrechen\n↑/↓ oder PgUp/PgDn Namen prüfen",
    ),
    (
        " Resource cleanup results ",
        " Ergebnisse der Ressourcen-Bereinigung ",
    ),
    ("Removed", "Entfernt"),
    ("Skipped: {a0}", "Übersprungen: {a0}"),
    ("Failed: {a0}", "Fehlgeschlagen: {a0}"),
    (
        "j/k PgUp/PgDn Scroll · Enter/Esc Close",
        "j/k PgUp/PgDn Scrollen · Enter/Esc Schließen",
    ),
    (
        "Only unreferenced resources can be marked for cleanup",
        "Nur ungenutzte Ressourcen können zur Bereinigung markiert werden",
    ),
    (
        "Demo mode does not contact registries",
        "Der Demo-Modus kontaktiert keine Registries",
    ),
    (
        "Select a connected runtime first",
        "Zuerst eine verbundene Runtime auswählen",
    ),
    (
        "Registry updates require a Docker-compatible runtime",
        "Registry-Updates benötigen eine Docker-kompatible Runtime",
    ),
    (
        "Image inventory is unavailable for this runtime",
        "Image-Inventar ist für diese Runtime nicht verfügbar",
    ),
    (
        "No tagged images are visible to check",
        "Keine getaggten Images zum Prüfen sichtbar",
    ),
    (
        "Checking {a0} images against their registries…",
        "{a0} Images werden mit ihren Registries abgeglichen…",
    ),
    (
        "Another image pull is still running",
        "Ein anderes Image wird noch abgerufen",
    ),
    (
        "Select an image in the Images view to pull",
        "Zum Abrufen ein Image in der Images-Ansicht auswählen",
    ),
    (
        "This image has no tag to pull",
        "Dieses Image hat keinen Tag zum Abrufen",
    ),
    ("Pulling {a0}…", "{a0} wird abgerufen…"),
    (
        " Security images · m/M Check · Enter/Tab Findings ",
        " Images scannen · m/M Markieren · Enter/Tab Befunde ",
    ),
    (" Images overview ", " Images-Übersicht "),
    (" Volumes overview ", " Volumes-Übersicht "),
    ("Select a runtime.", "Runtime auswählen."),
    (
        "Runtime unavailable; inventory cannot be read.",
        "Runtime nicht verfügbar; Inventar kann nicht gelesen werden.",
    ),
    (
        "{a0}  ·  {a1}  ·  Filter: {a2}  ·  Sort: {a3}{a4}",
        "{a0}  ·  {a1}  ·  Filter: {a2}  ·  Sortierung: {a3}{a4}",
    ),
    ("Bind mounts", "Bind-Mounts"),
    ("inventory", "Inventar"),
    ("  ·  STALE snapshot", "  ·  VERALTETER Snapshot"),
    (
        " · {count} ↑ update available",
        " · {count} ↑ Update verfügbar",
    ),
    ("{a0} checked for scan", "{a0} für Scan markiert"),
    (
        "{a0} cleanup / {a1} scan checked",
        "{a0} zur Bereinigung / {a1} für Scan markiert",
    ),
    ("{count} marked", "{count} markiert"),
    (
        "{a0} / {total} shown · {used} used · {unused} unreferenced · {marks}{updates}{a1}",
        "{a0} / {total} angezeigt · {used} genutzt · {unused} ohne Referenz · {marks}{updates}{a1}",
    ),
    (
        "Space/m Check · M All · s/S Scan · Enter Findings · Tab Focus · z Zoom",
        "Leertaste/m Markieren · M Alle · s/S Scan · Enter Befunde · Tab Fokus · z Zoom",
    ),
    (
        "Row {row} / {total} · U Check updates · :pull-image · m/M Check scan/all · 7 Security · :scan-images · z Zoom · Space Cleanup · d Delete · p Prune all unused · Enter Details",
        "Zeile {row} / {total} · U Updates prüfen · :pull-image · m/M Scan markieren/alle · 7 Sicherheit · :scan-images · z Zoom · Leertaste Bereinigung · d Löschen · p Alle ungenutzten bereinigen · Enter Details",
    ),
    (
        "Row {row} / {total} · Space Mark  u Mark unused  c Clear  d Delete  p Prune all unused  U Check updates  :pull-image · / Search  f Filter  s Sort  Enter Details",
        "Zeile {row} / {total} · Leertaste Markieren  u Ungenutzte markieren  c Leeren  d Löschen  p Alle ungenutzten bereinigen  U Updates prüfen  :pull-image · / Suchen  f Filter  s Sortieren  Enter Details",
    ),
    (
        "Row {row} / {total} · Space Mark  u Mark unused  c Clear  d Delete  p Prune all unused · / Search  f Filter  s Sort  Enter Details",
        "Zeile {row} / {total} · Leertaste Markieren  u Ungenutzte markieren  c Leeren  d Löschen  p Alle ungenutzten bereinigen · / Suchen  f Filter  s Sortieren  Enter Details",
    ),
    (
        "Inventory unavailable for this runtime. See Containers overview warnings.",
        "Inventar für diese Runtime nicht verfügbar. Siehe Warnungen in der Container-Übersicht.",
    ),
    (
        "No resources reported by this runtime.",
        "Diese Runtime meldet keine Ressourcen.",
    ),
    (
        "No matches. / edits search; f changes the status filter.",
        "Keine Treffer. / bearbeitet die Suche; f ändert den Statusfilter.",
    ),
    ("Image", "Image"),
    ("Volume / source", "Volume / Quelle"),
    ("Tags", "Tags"),
    ("Driver", "Treiber"),
    ("Size", "Größe"),
    ("Status", "Status"),
    ("Refs", "Refs"),
    ("Project", "Projekt"),
    ("Image / tags", "Image / Tags"),
    (" Details · Esc Back ", " Details · Esc Zurück "),
    ("ID / key: {a0}", "ID / Schlüssel: {a0}"),
    (
        "Status: {a0} · {a1} running / {a2} stopped",
        "Status: {a0} · {a1} läuft / {a2} gestoppt",
    ),
    ("Size: {a0}", "Größe: {a0}"),
    (
        "unknown (no filesystem scan)",
        "unbekannt (kein Dateisystem-Scan)",
    ),
    ("Update: {a0}", "Update: {a0}"),
    (
        "not checked · U compares with the registry",
        "nicht geprüft · U vergleicht mit der Registry",
    ),
    ("Container references:", "Container-Referenzen:"),
    ("  None reported", "  Keine gemeldet"),
    (
        "Sizes may include shared layers, not reclaimable space. Untagged does not imply unused.",
        "Größen können geteilte Layer enthalten und zeigen nicht den freiwerdenden Speicher. Ohne Tag heißt nicht ungenutzt.",
    ),
    (
        "Unreferenced does not mean safe to delete. Bind mounts are not managed volumes.",
        "Ohne Referenz heißt nicht, dass Löschen sicher ist. Bind-Mounts sind keine verwalteten Volumes.",
    ),
    (
        "Tha\nTerminal too small. Resize to at least 32 × 10.\nPress q to quit.",
        "Tha\nTerminal zu klein. Auf mindestens 32 × 10 vergrößern.\nq drücken zum Beenden.",
    ),
    ("Containers", "Container"),
    ("Images", "Images"),
    ("Security", "Sicherheit"),
    (" ⊤ Tha ", " ⊤ Tha "),
    (" Demo · ", " Demo · "),
    (
        "{connected} connected / {a0} configured",
        "{connected} verbunden / {a0} konfiguriert",
    ),
    (
        " Ctrl+←/→ or F7/F8 Overview · ←/→ Runtime · ↑/↓ Select · / Search · Enter Details",
        " Strg+←/→ oder F7/F8 Übersicht · ←/→ Runtime · ↑/↓ Auswählen · / Suchen · Enter Details",
    ),
    (
        " All runtimes › {a0}{a1} › {a2}  · Backspace Back",
        " Alle Runtimes › {a0}{a1} › {a2}  · Backspace Zurück",
    ),
    (
        " All runtimes · Enter a Compose project to focus it",
        " Alle Runtimes · Compose-Projekt öffnen, um es zu fokussieren",
    ),
    (" Export retained logs ", " Gespeicherte Logs exportieren "),
    (
        "Resize to review this dialog (at least 60 × 18). Esc cancels.",
        "Zum Prüfen dieses Dialogs vergrößern (mindestens 60 × 18). Esc bricht ab.",
    ),
    (
        "{} clears filters. Supported keys: source, level, min_level, fields, since, until.\nExample: {\"source\":\"api\",\"min_level\":\"warn\",\"fields\":{\"/http/status\":500}}\nTime bounds: RFC3339, e.g. \"since\":\"2026-10-29T10:00:00Z\".\nMissing required metadata does not match. Search remains separate.\nOnly retained entries are filtered; no older logs are fetched.",
        "{} löscht die Filter. Unterstützte Schlüssel: source, level, min_level, fields, since, until.\nBeispiel: {\"source\":\"api\",\"min_level\":\"warn\",\"fields\":{\"/http/status\":500}}\nZeitgrenzen: RFC3339, z. B. \"since\":\"2026-10-29T10:00:00Z\".\nEinträge ohne die nötigen Metadaten passen nicht. Die Suche bleibt getrennt.\nGefiltert werden nur gespeicherte Einträge; ältere Logs werden nicht nachgeladen.",
    ),
    (
        "Enter Export snapshot · Esc Cancel · Ctrl-u Clear",
        "Enter Snapshot exportieren · Esc Abbrechen · Strg-u Leeren",
    ),
    (
        "Resize to at least 60 × 18 to review lifecycle targets.\nEsc cancels confirmation; running actions continue.",
        "Auf mindestens 60 × 18 vergrößern, um die Ziele zu prüfen.\nEsc bricht die Bestätigung ab; laufende Aktionen gehen weiter.",
    ),
    (" Lifecycle ", " Aktion "),
    ("Results", "Ergebnisse"),
    ("Running", "Läuft"),
    (
        " ↑/↓ PgUp/PgDn Review targets ",
        " ↑/↓ PgUp/PgDn Ziele prüfen ",
    ),
    (
        "{a0} RUNTIME · ALL workloads",
        "{a0} RUNTIME · ALLE Workloads",
    ),
    (
        "{a0}: {a1} · {a2} target(s), {a3} skipped",
        "{a0}: {a1} · {a2} Ziel(e), {a3} übersprungen",
    ),
    ("Endpoint: {a0}", "Endpunkt: {a0}"),
    ("not currently available", "derzeit nicht verfügbar"),
    ("Provider target", "Anbieter-Ziel"),
    ("Runtime ID", "Runtime-ID"),
    (
        "RUNTIME-WIDE: affects ALL workloads, including outside the focused project.",
        "GESAMTE RUNTIME: betrifft ALLE Workloads, auch außerhalb des fokussierten Projekts.",
    ),
    ("Command: {program} {a0}", "Befehl: {program} {a0}"),
    (
        "Runtime configuration is retained; stopped runtimes remain (offline).",
        "Runtime-Konfiguration bleibt erhalten; gestoppte Runtimes bleiben sichtbar (offline).",
    ),
    (
        "Pending / checking runtime",
        "Ausstehend / Runtime wird geprüft",
    ),
    (
        "Existing containers only; no Compose dependency orchestration.",
        "Nur vorhandene Container; keine Compose-Abhängigkeitssteuerung.",
    ),
    (
        "Container writable data is lost. Networks, ALL volumes and images stay.",
        "Beschreibbare Container-Daten gehen verloren. Netzwerke, ALLE Volumes und Images bleiben.",
    ),
    (
        "10s stop grace period, then runtime may kill the process.",
        "10 s Wartezeit beim Stoppen, danach kann die Runtime den Prozess beenden.",
    ),
    (
        "Pending / checking safety",
        "Ausstehend / Sicherheit wird geprüft",
    ),
    (
        "Enter/Esc Close · Inventory refresh requested\nNo automatic retry or rollback; inspect unknown outcomes.",
        "Enter/Esc Schließen · Inventar-Aktualisierung angefordert\nKeine automatische Wiederholung oder Rücknahme; unbekannte Ergebnisse prüfen.",
    ),
    (
        "Enter/Esc Close · Actions continue in the background.\nQuitting cannot undo a request already sent to the runtime.",
        "Enter/Esc Schließen · Aktionen laufen im Hintergrund weiter.\nBeenden macht bereits an die Runtime gesendete Anfragen nicht rückgängig.",
    ),
    (
        "Resize to at least 44 × 10 to confirm.\nEsc Cancel",
        "Zum Bestätigen auf mindestens 44 × 10 vergrößern.\nEsc Abbrechen",
    ),
    (" Confirm action ", " Aktion bestätigen "),
    (
        "Starts this runtime; workloads may auto-start.",
        "Startet diese Runtime; Workloads können automatisch starten.",
    ),
    (
        "Stops this runtime and all workloads.",
        "Stoppt diese Runtime und alle Workloads.",
    ),
    (
        "Restarts this runtime and all workloads.",
        "Startet diese Runtime und alle Workloads neu.",
    ),
    (
        "Includes all OrbStack Linux machines.",
        "Enthält alle OrbStack-Linux-Maschinen.",
    ),
    (
        "Writable data is lost; all volumes/images stay.",
        "Beschreibbare Daten gehen verloren; alle Volumes/Images bleiben.",
    ),
    (
        "{a0} target(s), {a1} skipped",
        "{a0} Ziel(e), {a1} übersprungen",
    ),
    (
        "Type {a0} then Enter to DELETE\n> {a1} · Esc Cancel",
        "Zum LÖSCHEN {a0} eingeben und Enter drücken\n> {a1} · Esc Abbrechen",
    ),
    (
        "y / Enter Confirm    n / Esc Cancel",
        "y / Enter Bestätigen    n / Esc Abbrechen",
    ),
    ("Target: {a0}", "Ziel: {a0}"),
    ("↑/↓ Review targets", "↑/↓ Ziele prüfen"),
    (
        " ▸ Performance · Selected container (p/P)",
        " ▸ Performance · Ausgewählter Container (p/P)",
    ),
    (
        " ▸ Performance · Connected runtimes (p/P)",
        " ▸ Performance · Verbundene Runtimes (p/P)",
    ),
    (
        " ▾ Performance · Selected container · P Runtime ",
        " ▾ Performance · Ausgewählter Container · P Runtime ",
    ),
    (
        " ▾ Performance · Connected runtimes · P Container ",
        " ▾ Performance · Verbundene Runtimes · P Container ",
    ),
    (
        " j/k Select · Enter Inspect · z Zoom · p Close ",
        " j/k Auswählen · Enter Ansehen · z Zoom · p Schließen ",
    ),
    (
        "Select a container in Explorer · P Runtime",
        "Container im Explorer auswählen · P Runtime",
    ),
    ("No connected runtimes", "Keine verbundenen Runtimes"),
    (" Explorer ", " Explorer "),
    (" Explorer / {a0} ", " Explorer / {a0} "),
    (
        "No matching workloads. Esc clears search.",
        "Keine passenden Workloads. Esc löscht die Suche.",
    ),
    ("Workload", "Workload"),
    ("Mem", "Speicher"),
    ("1 Overview", "1 Übersicht"),
    ("2 Logs", "2 Logs"),
    ("3 Networks", "3 Netzwerke"),
    ("4 Exec", "4 Exec"),
    ("5 Events", "5 Ereignisse"),
    ("6 Metrics", "6 Metriken"),
    ("7 Layers", "7 Layer"),
    ("8 Security", "8 Sicherheit"),
    ("1 Info", "1 Info"),
    ("3 Net", "3 Netz"),
    ("SHELL INPUT", "SHELL-EINGABE"),
    ("OUTPUT RETAINED", "AUSGABE GESPEICHERT"),
    (
        "No session\nNAVIGATION · Select a container and press e",
        "Keine Sitzung\nNAVIGATION · Container auswählen und e drücken",
    ),
    ("No selection", "Keine Auswahl"),
    ("Focused", "Im Fokus"),
    ("Navigation", "Navigation"),
    (
        "Exec resize failed: {error}",
        "Exec-Größenänderung fehlgeschlagen: {error}",
    ),
    (
        "Select one connected container or a one-container service.\nRead-only image metadata; no pulls, extraction, or builds.",
        "Einen verbundenen Container oder einen Service mit einem Container auswählen.\nNur Image-Metadaten lesen; kein Abruf, Entpacken oder Build.",
    ),
    (
        "Loading resolved image layers… (read-only)",
        "Image-Layer werden geladen… (nur lesend)",
    ),
    (
        "Layers unavailable: {a0}\n:reload-layers retries; no image will be pulled.",
        "Layer nicht verfügbar: {a0}\n:reload-layers versucht es erneut; es wird kein Image abgerufen.",
    ),
    ("Image: {a0}", "Image: {a0}"),
    (
        "Filesystem layers ({a0}) · Build history ({a1})\nBase → top · separate sections, not a guessed layer/step mapping",
        "Dateisystem-Layer ({a0}) · Build-Verlauf ({a1})\nBasis → oben · getrennte Abschnitte, keine geratene Zuordnung von Layer zu Schritt",
    ),
    ("Filesystem", "Dateisystem"),
    ("unknown", "unbekannt"),
    ("Metadata", "Metadaten"),
    ("Build step", "Build-Schritt"),
    (
        "instruction hidden · Enter details",
        "Anweisung verborgen · Enter Details",
    ),
    ("Kind", "Art"),
    ("Diff ID / instruction", "Diff-ID / Anweisung"),
    (
        "j/k Select · Enter Details · :reload-layers Retry",
        "j/k Auswählen · Enter Details · :reload-layers Wiederholen",
    ),
    ("Layer details", "Layer-Details"),
    ("Resolved image: {a0}", "Aufgelöstes Image: {a0}"),
    (
        "Enter/Esc Back · j/k/PgUp/PgDn Scroll",
        "Enter/Esc Zurück · j/k/PgUp/PgDn Scrollen",
    ),
    ("Filesystem diff ID: {id}", "Dateisystem-Diff-ID: {id}"),
    (
        "Uncompressed diff ID; size and build-step association are unavailable.",
        "Unkomprimierte Diff-ID; Größe und Zuordnung zum Build-Schritt sind nicht verfügbar.",
    ),
    (
        "Build history may contain secrets. Instructions are never executed.",
        "Der Build-Verlauf kann Geheimnisse enthalten. Anweisungen werden nie ausgeführt.",
    ),
    (
        "Explicit metadata-only step",
        "Als reiner Metadaten-Schritt ausgewiesen",
    ),
    (
        "Explicit filesystem-producing step",
        "Als Schritt mit Dateisystem-Änderung ausgewiesen",
    ),
    (
        "Filesystem association unavailable (not inferred from size)",
        "Dateisystem-Zuordnung nicht verfügbar (nicht aus der Größe abgeleitet)",
    ),
    (
        "Reported history size: {a0}",
        "Gemeldete Verlaufsgröße: {a0}",
    ),
    ("Instruction:", "Anweisung:"),
    ("Comment:", "Kommentar:"),
    (
        "Embedded container shell\n\nSelect a running container or service and press e.\n{a0} returns keyboard focus to Tha without closing the shell.\nThe session target stays fixed when you browse other containers.",
        "Eingebettete Container-Shell\n\nLaufenden Container oder Service auswählen und e drücken.\n{a0} gibt den Tastaturfokus an Tha zurück, ohne die Shell zu schließen.\nDas Sitzungsziel bleibt gleich, auch beim Ansehen anderer Container.",
    ),
    ("Container · {a0} · {a1}", "Container · {a0} · {a1}"),
    ("CPU {a0} · RAM {a1}", "CPU {a0} · RAM {a1}"),
    (
        "z Zoom · more room for history",
        "z Zoom · mehr Platz für den Verlauf",
    ),
    ("{a0} · CPU {a1}", "{a0} · CPU {a1}"),
    ("CPU {cpu} · scale {scale}%", "CPU {cpu} · Skala {scale}%"),
    ("Memory {a0}{a1}", "Speicher {a0}{a1}"),
    (
        "Container usage / memory limit",
        "Container-Nutzung / Speicherlimit",
    ),
    (
        "Container usage / runtime capacity",
        "Container-Nutzung / Runtime-Kapazität",
    ),
    (
        "Capacity or live usage unavailable",
        "Kapazität oder aktuelle Nutzung nicht verfügbar",
    ),
    (
        "{a0} · {a1}s target · latest {a2}s ago · · gap{a3}",
        "{a0} · Ziel {a1}s · zuletzt vor {a2}s · · Lücke{a3}",
    ),
    ("Container", "Container"),
    ("Container totals", "Container gesamt"),
    (" · offline", " · offline"),
    ("{a0} · CPU {a1}{a2}", "{a0} · CPU {a1}{a2}"),
    (" · Mem {a0}", " · Speicher {a0}"),
    (
        "Alias of {a0} · not counted twice",
        "Alias von {a0} · nicht doppelt gezählt",
    ),
    (
        "Unavailable · no live measurements",
        "Nicht verfügbar · keine aktuellen Messwerte",
    ),
    ("Connecting…", "Verbinde…"),
    (
        "{memory} · capacity unavailable",
        "{memory} · Kapazität nicht verfügbar",
    ),
    (
        "{target}s target · CPU {cpu}/{running} Mem {memory}/{running} · scale {scale}%",
        "Ziel {target}s · CPU {cpu}/{running} Speicher {memory}/{running} · Skala {scale}%",
    ),
    (
        "Select a runtime or workload in the explorer.",
        "Runtime oder Workload im Explorer auswählen.",
    ),
    (
        "Same engine as {a0}. Browse that connection to avoid duplicates.",
        "Dieselbe Engine wie {a0}. Diese Verbindung öffnen, um Duplikate zu vermeiden.",
    ),
    ("Connection unavailable", "Verbindung nicht verfügbar"),
    (
        "Showing stale, last-known data below.",
        "Unten stehen veraltete, zuletzt bekannte Daten.",
    ),
    ("Runtime control: {a0}", "Runtime-Steuerung: {a0}"),
    (
        "S Start runtime · s Stop runtime · r Restart runtime (ALL workloads)",
        "S Runtime starten · s Runtime stoppen · r Runtime neu starten (ALLE Workloads)",
    ),
    (
        "Runtime control unsupported; select a workload for container actions.",
        "Runtime-Steuerung nicht unterstützt; für Container-Aktionen einen Workload auswählen.",
    ),
    (
        "Connecting… Other engines remain usable.",
        "Verbinde… Andere Engines bleiben nutzbar.",
    ),
    ("Version      {a0}", "Version      {a0}"),
    ("Endpoint     {a0}", "Endpunkt     {a0}"),
    (
        "Capacity     {a0} CPUs · {a1} memory (not current host usage)",
        "Kapazität    {a0} CPUs · {a1} Speicher (nicht die aktuelle Host-Nutzung)",
    ),
    ("Ownership    {a0}", "Eigentümer   {a0}"),
    ("No project label", "Kein Projekt-Label"),
    (
        "Shared/external resource; not exclusive to this project.",
        "Geteilte/externe Ressource; nicht exklusiv für dieses Projekt.",
    ),
    ("Attached containers", "Angebundene Container"),
    (
        "No attachments reported.",
        "Keine angebundenen Container gemeldet.",
    ),
    (
        "{a0} attachments outside this project (hidden)",
        "{a0} Container außerhalb dieses Projekts (ausgeblendet)",
    ),
    (
        "{a0} containers · {a1} running",
        "{a0} Container · {a1} läuft",
    ),
    ("Workload CPU     {a0}", "Workload-CPU      {a0}"),
    ("Workload memory  {a0}", "Workload-Speicher {a0}"),
    ("  Memory     {a0}{a1}", "  Speicher   {a0}{a1}"),
    (" / {a0} reported limit", " / {a0} gemeldetes Limit"),
    ("  Stats      {a0}", "  Stats      {a0}"),
    (
        "  Last sample {a0}s ago · CPU {a1} · memory {a2} (stale, excluded from totals)",
        "  Letzte Messung vor {a0}s · CPU {a1} · Speicher {a2} (veraltet, nicht in Summen)",
    ),
    ("  Image      {a0}", "  Image      {a0}"),
    ("unavailable · {error}", "nicht verfügbar · {error}"),
    ("unavailable (disconnected)", "nicht verfügbar (getrennt)"),
    ("loading image metadata…", "Image-Metadaten werden geladen…"),
    ("  Platform   {a0}", "  Plattform  {a0}"),
    ("  Project    {a0}", "  Projekt    {a0}"),
    ("  Service    {a0}", "  Service    {a0}"),
    ("  Port       {a0}", "  Port       {a0}"),
    (
        "  Last-known bindings (offline); localhost reachability not verified.",
        "  Zuletzt bekannte Bindungen (offline); Erreichbarkeit über localhost nicht geprüft.",
    ),
    (
        "  Container is stopped; published ports are not active.",
        "  Container ist gestoppt; veröffentlichte Ports sind nicht aktiv.",
    ),
    (
        "  Reported bindings; localhost/VM forwarding reachability not verified.",
        "  Gemeldete Bindungen; Erreichbarkeit über localhost/VM-Weiterleitung nicht geprüft.",
    ),
    (
        "  Ports      No port mappings reported",
        "  Ports      Kein Port zugeordnet",
    ),
    (
        "  Port       {a0} (reported)",
        "  Port       {a0} (gemeldet)",
    ),
    ("  Network    {a0}", "  Netzwerk   {a0}"),
    ("  Volume     {a0}", "  Volume     {a0}"),
    (
        "  Kubernetes-managed · inspection only",
        "  Von Kubernetes verwaltet · nur Ansicht",
    ),
    (
        "No containers reported in this scope.",
        "Keine Container in diesem Bereich gemeldet.",
    ),
    ("Images ({a0})", "Images ({a0})"),
    ("size unavailable", "Größe nicht verfügbar"),
    (
        "S Start · s Stop · r Restart · d Delete (Explorer; confirmation required)",
        "S Starten · s Stoppen · r Neu starten · d Löschen (Explorer; Bestätigung nötig)",
    ),
    ("Network membership", "Netzwerkzuordnung"),
    (
        "Browse {a0} for this engine's resources.",
        "{a0} öffnen, um die Ressourcen dieser Engine zu sehen.",
    ),
    (
        "Connection unavailable / stale · see Overview for details",
        "Verbindung nicht verfügbar / veraltet · Details in der Übersicht",
    ),
    (
        "Native containerd network inventory is not supported by this MVP adapter.",
        "Natives containerd-Netzwerkinventar wird von diesem MVP-Adapter nicht unterstützt.",
    ),
    (
        "Networks depend on the launcher and its CNI configuration.",
        "Netzwerke hängen vom Launcher und seiner CNI-Konfiguration ab.",
    ),
    (
        "Engine has not returned an inventory.",
        "Engine hat kein Inventar geliefert.",
    ),
    ("Standalone", "Einzelcontainer"),
    (
        "  No attachments reported",
        "  Keine angebundenen Container gemeldet",
    ),
    (
        "  {hidden} attachments outside this project (hidden)",
        "  {hidden} Container außerhalb dieses Projekts (ausgeblendet)",
    ),
    (
        "No networks reported for this selection.",
        "Für diese Auswahl wurden keine Netzwerke gemeldet.",
    ),
    ("Rows {a0}–{end} / {total}", "Zeilen {a0}–{end} / {total}"),
    (" · Since {at}", " · Seit {at}"),
    (
        "Wrap: {a0} · JSON: {a1} · {a2} · {position}{since}",
        "Umbruch: {a0} · JSON: {a1} · {a2} · {position}{since}",
    ),
    ("on", "an"),
    ("off", "aus"),
    ("pretty", "formatiert"),
    ("raw", "roh"),
    ("Following", "Live"),
    (
        "{a0}/{a1} entries retained · Filter: {a2} · {a3} evicted",
        "{a0}/{a1} Einträge gespeichert · Filter: {a2} · {a3} verworfen",
    ),
    (
        "{a0}/{a1} matches {a2}/{a3} · Filter: {a4}",
        "{a0}/{a1} Treffer {a2}/{a3} · Filter: {a4}",
    ),
    ("regex ", "Regex "),
    (
        "Waiting for logs. Select a container, service, or project.",
        "Warten auf Logs. Container, Service oder Projekt auswählen.",
    ),
    (
        "Metrics · {shorter}/{longer} Range  ←/→ Pan  c Cursor  h/l Inspect  f Live  z Enlarge  Esc Restore",
        "Metriken · {shorter}/{longer} Zeitraum  ←/→ Verschieben  c Cursor  h/l Ansehen  f Live  z Vergrößern  Esc Wiederherstellen",
    ),
    (
        "Log dialog · Enter Apply/Export · Esc Cancel",
        "Log-Dialog · Enter Anwenden/Exportieren · Esc Abbrechen",
    ),
    (
        "Search names, tags, IDs, projects and container references · Enter Apply · Esc Cancel · Ctrl-U Clear",
        "Sucht Namen, Tags, IDs, Projekte und Container-Referenzen · Enter Anwenden · Esc Abbrechen · Strg-U Leeren",
    ),
    (
        "Enter Apply · Esc Cancel · Ctrl-R {a0} · Case-insensitive",
        "Enter Anwenden · Esc Abbrechen · Strg-R {a0} · ignoriert Groß-/Kleinschreibung",
    ),
    ("Literal", "Literal"),
    ("Regex", "Regex"),
    (
        "Enter Apply · Esc Cancel · Searches the focused view",
        "Enter Anwenden · Esc Abbrechen · Durchsucht die fokussierte Ansicht",
    ),
    (
        "Security · s Scan · S Scan checked · x Cancel · Space/m Check · Tab Images/Findings · Enter Details · z Zoom",
        "Sicherheit · s Scan · S Markierte scannen · x Abbrechen · Leertaste/m Markieren · Tab Images/Befunde · Enter Details · z Zoom",
    ),
    (
        "Space Mark · u Unused · c Clear · d Delete · / Search · f Filter · s Sort · Enter Details · Esc Back",
        "Leertaste Markieren · u Ungenutzt · c Leeren · d Löschen · / Suchen · f Filter · s Sortieren · Enter Details · Esc Zurück",
    ),
    (
        "Ctrl-] Navigate · SHELL INPUT · Keys go to the selected container session",
        "Strg-] Navigieren · SHELL-EINGABE · Tasten gehen an die ausgewählte Container-Sitzung",
    ),
    (
        "F12 Navigate · SHELL INPUT · Keys go to the selected container session",
        "F12 Navigieren · SHELL-EINGABE · Tasten gehen an die ausgewählte Container-Sitzung",
    ),
    (
        "Performance · P Scope  z Zoom  j/k Select  Enter Inspect  p Close  Tab Pane  ? Keys",
        "Performance · P Runtime/Container  z Zoom  j/k Auswählen  Enter Ansehen  p Schließen  Tab Wechseln  ? Tasten",
    ),
    (
        "Security · j/k Select  Enter Details  z Zoom  Tab Explorer · Scans run in the Security view",
        "Sicherheit · j/k Auswählen  Enter Details  z Zoom  Tab Explorer · Scans laufen in der Sicherheitsansicht",
    ),
    (
        "F6 Sessions · Enter Input · :close-exec Close slot · j/k Scroll · Tab Explorer · ? Keys",
        "F6 Sitzungen · Enter Eingabe · :close-exec Platz freigeben · j/k Scrollen · Tab Explorer · ? Tasten",
    ),
    (
        "? Keys  F3 Filter  :logs-since  J JSON  w Wrap  / Search  n/N Match  j/k Scroll  f Follow  Tab Explorer",
        "? Tasten  F3 Filter  :logs-since  J JSON  w Umbruch  / Suchen  n/N Treffer  j/k Scrollen  f Live  Tab Explorer",
    ),
    (
        "Ctrl+←/→ Overview  ? Keys  a Actions  p Performance  Enter Open  e Exec  Tab Pane  z Zoom  1..8 Details  : Commands",
        "Strg+←/→ Übersicht  ? Tasten  a Aktionen  p Performance  Enter Öffnen  e Exec  Tab Wechseln  z Zoom  1..8 Details  : Befehle",
    ),
    ("Activity · {status}", "Aktivität · {status}"),
    ("unbound", "nicht belegt"),
    (
        "Set a key to 'none' to disable it. Restart after editing config.toml.",
        "Taste auf 'none' setzen, um sie zu deaktivieren. Nach Änderungen an config.toml neu starten.",
    ),
    (
        "Configurable action names (built-in defaults; custom keys below take precedence)",
        "Konfigurierbare Aktionsnamen (eingebaute Standards; eigene Tasten unten haben Vorrang)",
    ),
    (
        "Custom navigation bindings (override defaults only in navigation)",
        "Eigene Navigationstasten (ersetzen Standards nur in der Navigation)",
    ),
    (
        "Exec and lifecycle actions can change workloads. Confirmations remain required.",
        "Exec und Aktionen können Workloads ändern. Bestätigung bleibt nötig.",
    ),
    (" Tha · Shortcuts / Cheatsheet ", " Tha · Tastenkürzel "),
    (
        " j/k Scroll · ? / Esc Close ",
        " j/k Scrollen · ? / Esc Schließen ",
    ),
    (
        "Enter Execute · Tab Complete · Esc Cancel",
        "Enter Ausführen · Tab Vervollständigen · Esc Abbrechen",
    ),
    (" Commands ", " Befehle "),
    (
        " j/k Select · Enter Choose · Esc Cancel ",
        " j/k Auswählen · Enter Wählen · Esc Abbrechen ",
    ),
    (
        "{count} existing container(s). Enter opens confirmation; nothing is sent now.",
        "Vorhandene Container: {count}. Enter öffnet die Bestätigung; jetzt wird nichts gesendet.",
    ),
    ("Disabled: {reason}", "Deaktiviert: {reason}"),
    (
        "Result no longer available",
        "Ergebnis nicht mehr verfügbar",
    ),
    ("#{id} · closed", "#{id} · geschlossen"),
    ("{count} target(s)", "{count} Ziel(e)"),
    ("disabled", "deaktiviert"),
    (
        "Install the security module: tha module install security",
        "Sicherheitsmodul installieren: tha module install security",
    ),
    (
        " Advisory details · Esc Back ",
        " Advisory-Details · Esc Zurück ",
    ),
    (" CVEs · saved report ", " CVEs · gespeicherter Bericht "),
    (
        " Vulnerabilities · saved report · z Expand/restore ",
        " Schwachstellen · gespeicherter Bericht · z Vergrößern/Zurück ",
    ),
    (
        " Vulnerabilities · z Expand/restore ",
        " Schwachstellen · z Vergrößern/Zurück ",
    ),
    (
        "Image only; excludes live container changes and mounted files.",
        "Nur das Image; ohne Live-Änderungen im Container und eingebundene Dateien.",
    ),
    (
        "This container's image ID is unknown, so no report can be matched.",
        "Die Image-ID dieses Containers ist unbekannt, daher passt kein Bericht.",
    ),
    (
        "No saved report yet. Scan this image from the Security view (Ctrl+→ or Alt+4).",
        "Noch kein gespeicherter Bericht. Dieses Image in der Sicherheitsansicht scannen (Strg+→ oder Alt+4).",
    ),
    (
        "No saved report yet · s Scan checked/selected images",
        "Noch kein gespeicherter Bericht · s Markierte/ausgewählte Images scannen",
    ),
    (
        "Module not installed. Run: tha module install security",
        "Modul nicht installiert. Ausführen: tha module install security",
    ),
    (
        "Image scan failed; this is NOT a clean scan.",
        "Image-Scan fehlgeschlagen; das heißt NICHT, dass keine Schwachstellen vorliegen.",
    ),
    ("Critical", "Kritisch"),
    ("High", "Hoch"),
    ("Medium", "Mittel"),
    ("Low", "Niedrig"),
    ("Negligible", "Vernachlässigbar"),
    ("Unknown", "Unbekannt"),
    ("unknown time", "unbekannte Zeit"),
    (
        "Grype {a0} · scanned {scanned} · saved report, not a live security guarantee",
        "Grype {a0} · gescannt {scanned} · gespeicherter Bericht, keine Live-Sicherheitsgarantie",
    ),
    (
        "{a0} · {a1} · CVSS {a2} (highest reported base score)",
        "{a0} · {a1} · CVSS {a2} (höchster gemeldeter Basiswert)",
    ),
    ("Installed  {a0}", "Installiert {a0}"),
    ("Fixed in   {a0}", "Behoben in {a0}"),
    ("No fix reported", "Kein Fix gemeldet"),
    (
        "Grype {a0} · database built {a1}",
        "Grype {a0} · Datenbank erstellt {a1}",
    ),
    ("not reported", "nicht gemeldet"),
    (
        "Esc/Enter Back to findings · j/k Scroll · z Expand/restore",
        "Esc/Enter Zurück zu den Befunden · j/k Scrollen · z Vergrößern/Zurück",
    ),
    (
        "No known matching vulnerabilities found within scanner coverage.",
        "Keine bekannten passenden Schwachstellen im Rahmen der Scanner-Abdeckung gefunden.",
    ),
    (
        "{a0} / {a1} · j/k Select · Enter Details · z Zoom",
        "{a0} / {a1} · j/k Auswählen · Enter Details · z Zoom",
    ),
    (" {elapsed} · x Cancel", " {elapsed} · x Abbrechen"),
    (
        "Target {a0} · saved CVEs remain browsable",
        "Ziel {a0} · gespeicherte CVEs bleiben einsehbar",
    ),
    (
        "Service-owned queue · safe to close the UI",
        "Warteschlange des Dienstes · UI kann gefahrlos geschlossen werden",
    ),
    ("Severity", "Schweregrad"),
    ("Advisory", "Advisory"),
    ("Package", "Paket"),
    ("Installed", "Installiert"),
    ("Fixed in", "Behoben in"),
    ("Sev", "Schwere"),
    ("Crit", "Krit"),
    ("Med", "Mit"),
    ("Neg", "Ver"),
    ("Unk", "Unb"),
    ("On", "An"),
    ("Off", "Aus"),
    (" Settings ", " Einstellungen "),
    (
        " j/k Select · h/l or Enter Change · Esc Close ",
        " j/k Auswählen · h/l oder Enter Ändern · Esc Schließen ",
    ),
    ("Theme", "Design"),
    ("Language", "Sprache"),
    ("Keyboard layout", "Tastaturlayout"),
    ("Mouse", "Maus"),
    ("Wrap logs", "Logs umbrechen"),
    ("Reset to defaults", "Auf Standard zurücksetzen"),
    (
        "Separate from the keyboard layout, so any language works with any layout.",
        "Unabhängig vom Tastaturlayout: jede Sprache funktioniert mit jedem Layout.",
    ),
    (
        "Hold Shift to select terminal text while the mouse is on.",
        "Shift halten, um Terminaltext auszuwählen, solange die Maus an ist.",
    ),
    (
        "The same setting as w in the log pane.",
        "Dieselbe Einstellung wie w im Log-Bereich.",
    ),
    (
        "Enter returns these settings to config.toml or the defaults.",
        "Enter setzt diese Einstellungen auf config.toml oder die Standardwerte zurück.",
    ),
    (
        "Could not save settings",
        "Einstellungen konnten nicht gespeichert werden",
    ),
    (
        "Changes last for this session: demo mode or saving is turned off.",
        "Änderungen gelten für diese Sitzung: Demo-Modus oder Speichern ist deaktiviert.",
    ),
    (
        "Colors are off: NO_COLOR, --monochrome or :monochrome.",
        "Farben sind aus: NO_COLOR, --monochrome oder :monochrome.",
    ),
    (
        "Your terminal does not report truecolor, so this theme may look wrong.",
        "Das Terminal meldet kein Truecolor, dieses Design sieht vielleicht falsch aus.",
    ),
    (
        " j/k Preview · Enter Apply · Esc Cancel ",
        " j/k Vorschau · Enter Anwenden · Esc Abbrechen ",
    ),
    (
        "Review background lifecycle progress and results",
        "Fortschritt und Ergebnisse von Container-Aktionen im Hintergrund ansehen",
    ),
    ("Show workload overview", "Workload-Übersicht anzeigen"),
    (
        "Prune all unreferenced resources in Images/Volumes (confirmation required)",
        "Alle ungenutzten Ressourcen in Images/Volumes bereinigen (Bestätigung nötig)",
    ),
    (
        "Review the last image/volume cleanup results",
        "Letzte Ergebnisse der Image-/Volume-Bereinigung ansehen",
    ),
    (
        "Analyze the selected container's CPU, RAM, disk and network metrics (6)",
        "CPU-, RAM-, Disk- und Netzwerk-Metriken des ausgewählten Containers analysieren (6)",
    ),
    (
        "Switch to Images overview (Alt/Option+2; guarded unused-image removal)",
        "Zur Images-Übersicht wechseln (Alt/Option+2; geschütztes Entfernen ungenutzter Images)",
    ),
    (
        "Switch to Volumes overview (Alt/Option+3; guarded unused-volume removal)",
        "Zur Volumes-Übersicht wechseln (Alt/Option+3; geschütztes Entfernen ungenutzter Volumes)",
    ),
    (
        "Alias for metrics: live and historical container readings (6)",
        "Alias für metrics: aktuelle und historische Container-Messwerte (6)",
    ),
    (
        "Open logs for the selected workload",
        "Logs des ausgewählten Workloads öffnen",
    ),
    ("Show network membership", "Netzwerkzuordnung anzeigen"),
    (
        "Refresh all engine snapshots",
        "Alle Engine-Snapshots aktualisieren",
    ),
    (
        "Reload the current logs now; streams also reconnect automatically",
        "Aktuelle Logs jetzt neu laden; Streams verbinden sich auch automatisch neu",
    ),
    (
        "Load logs from a start time: 15m, 2h, 1d, RFC 3339, or off",
        "Logs ab einer Startzeit laden: 15m, 2h, 1d, RFC 3339 oder off",
    ),
    (
        "Toggle log wrapping (w in logs)",
        "Log-Umbruch umschalten (w in Logs)",
    ),
    (
        "Pretty-print or restore JSON logs (J in logs)",
        "JSON-Logs formatieren oder wiederherstellen (J in Logs)",
    ),
    (
        "Open a shell in the selected container",
        "Shell im ausgewählten Container öffnen",
    ),
    (
        "Inspect resolved image filesystem layers and build history (7)",
        "Aufgelöste Dateisystem-Layer und Build-Verlauf des Images ansehen (7)",
    ),
    (
        "Retry image layers lookup (read-only)",
        "Abfrage der Image-Layer wiederholen (nur lesend)",
    ),
    (
        "Show the selected image's saved security report (8; z expands)",
        "Gespeicherten Sicherheitsbericht des ausgewählten Images anzeigen (8; z vergrößert)",
    ),
    (
        "Scan checked images, or the selected image when none are checked",
        "Markierte Images scannen, oder das ausgewählte, wenn keins markiert ist",
    ),
    (
        "Scan checked images sequentially; continue after image errors",
        "Markierte Images nacheinander scannen; bei Image-Fehlern weitermachen",
    ),
    (
        "Cancel the security queue and remove its scanner",
        "Sicherheits-Warteschlange abbrechen und ihren Scanner entfernen",
    ),
    (
        "Refresh scanner/database and rescan checked or selected images",
        "Scanner/Datenbank aktualisieren und markierte oder ausgewählte Images neu scannen",
    ),
    (
        "Show contextual actions and disabled reasons (a)",
        "Kontextbezogene Aktionen und Gründe für deaktivierte anzeigen (a)",
    ),
    (
        "Close the embedded shell (not the container)",
        "Eingebettete Shell schließen (nicht den Container)",
    ),
    (
        "Switch retained Exec sessions (F6 in navigation)",
        "Zwischen offenen Exec-Sitzungen wechseln (F6 in der Navigation)",
    ),
    (
        "Export matching retained logs to a new private JSONL file",
        "Passende gespeicherte Logs in eine neue private JSONL-Datei exportieren",
    ),
    (
        "Compare visible tagged images with their registries (U in Images)",
        "Sichtbare getaggte Images mit ihren Registries vergleichen (U in Images)",
    ),
    (
        "Pull the selected image's tags; running containers are unchanged",
        "Tags des ausgewählten Images abrufen; laufende Container bleiben unverändert",
    ),
    (
        "Show container starts, exits, OOM kills and health changes (5)",
        "Container-Starts, Beendigungen, OOM-Kills und Health-Änderungen anzeigen (5)",
    ),
    (
        "Pull newer images, then apply them with up -d (confirmed)",
        "Neuere Images abrufen und dann mit up -d anwenden (mit Bestätigung)",
    ),
    (
        "Pull the project's images (confirmed)",
        "Images des Projekts abrufen (mit Bestätigung)",
    ),
    (
        "Apply Compose configuration with up -d (confirmed)",
        "Compose-Konfiguration mit up -d anwenden (mit Bestätigung)",
    ),
    (
        "Recreate every container with --force-recreate (confirmed)",
        "Jeden Container mit --force-recreate neu erstellen (mit Bestätigung)",
    ),
    (
        "Remove containers and networks, keeping volumes (type name)",
        "Container und Netzwerke entfernen, Volumes behalten (Name eingeben)",
    ),
    (
        "Show the last Compose operation's output",
        "Ausgabe des letzten Compose-Vorgangs anzeigen",
    ),
    (
        "Expand/collapse global performance (p)",
        "Globale Performance auf-/zuklappen (p)",
    ),
    (
        "Sort contexts by connection, name, CPU or memory (o)",
        "Kontexte nach Verbindung, Name, CPU oder Speicher sortieren (o)",
    ),
    (
        "Return to the previous workspace",
        "Zum vorherigen Workspace zurückkehren",
    ),
    ("Return to all runtimes", "Zu allen Runtimes zurückkehren"),
    ("Toggle terminal colors", "Terminalfarben umschalten"),
    (
        "Theme, language, keyboard layout, mouse, log wrapping (,)",
        "Design, Sprache, Tastaturlayout, Maus, Log-Umbruch (,)",
    ),
    ("Show keyboard shortcuts", "Tastenkürzel anzeigen"),
    ("Exit Tha", "Tha beenden"),
    ("Navigation\n", "Navigation\n"),
    (
        "j / k, arrows     Move in the focused pane\n",
        "j / k, Pfeile     Im fokussierten Bereich bewegen\n",
    ),
    (
        "h / l             Collapse/parent, expand/child (tree)\n",
        "h / l             Einklappen/hoch, Ausklappen/tiefer (Baum)\n",
    ),
    (
        "Space             Toggle the selected explorer branch\n",
        "Leertaste         Ausgewählten Explorer-Zweig umschalten\n",
    ),
    (
        "Enter             Enter a Compose workspace; otherwise open details\n",
        "Enter             Compose-Workspace öffnen; sonst Details öffnen\n",
    ),
    (
        "Backspace         Return to the previous workspace\n",
        "Backspace         Zum vorherigen Workspace zurück\n",
    ),
    (
        "Esc               Clear search, return to explorer, then go back\n",
        "Esc               Suche löschen, zum Explorer, dann zurück\n",
    ),
    (
        "q                 Back through panes/workspaces; quit from home\n",
        "q                 Zurück durch Bereiche/Workspaces; Startseite beendet\n",
    ),
    (
        "Tab / Shift-Tab   Cycle Explorer / Performance (open) / Details\n",
        "Tab / Shift-Tab   Explorer / Performance (offen) / Details durchlaufen\n",
    ),
    (
        "p                 Open/close Performance (compact by default)\n",
        "p                 Performance öffnen/schließen (standardmäßig kompakt)\n",
    ),
    (
        "P (Shift-p)       Switch Runtime / Selected container; open Performance\n",
        "P (Shift-p)       Runtime / ausgewählter Container; öffnet Performance\n",
    ),
    (
        "z                 Expand / restore the focused detail tab or Performance\n",
        "z                 Fokussierten Tab oder Performance vergrößern/zurück\n",
    ),
    (
        "Esc               Restore compact Performance before other navigation\n",
        "Esc               Kompakte Performance vor anderer Navigation herstellen\n",
    ),
    (
        "o                 Sort contexts (pins first; usage is a snapshot)\n",
        "o                 Kontexte sortieren (Fixierte zuerst; Momentaufnahme)\n",
    ),
    (
        "S / s / r / d     Start / Stop / Restart / Delete in Explorer\n",
        "S / s / r / d     Starten / Stoppen / Neu starten / Löschen im Explorer\n",
    ),
    (
        "                  Container or Compose rows only; confirmation required\n",
        "                  Nur Container- oder Compose-Zeilen; Bestätigung nötig\n",
    ),
    (
        "                  y/Enter confirms; n/Esc cancels; Delete: type name + Enter\n",
        "                  y/Enter ja; n/Esc nein; Löschen: Name + Enter\n",
    ),
    (
        "                  Delete stopped containers only; keep networks/volumes/images\n",
        "                  Nur gestoppte Container; Netze/Volumes/Images bleiben\n",
    ),
    (
        "1..8              Info Logs Net Exec Events Metrics Layers Security\n",
        "1..8              Info Logs Netz Exec Ereignisse Metriken Layer Sicherheit\n",
    ),
    (
        "Ctrl+←/→, F7/F8    Cycle overviews · Alt+1/2/3 direct\n",
        "Strg+←/→, F7/F8    Übersichten durchlaufen · Alt+1/2/3 direkt\n",
    ),
    (
        "a                 Contextual actions with disabled reasons (Explorer)\n",
        "a                 Kontextbezogene Aktionen mit Gründen (Explorer)\n",
    ),
    (
        "Enter in Layers   Expand details; Esc returns; no image pulls\n",
        "Enter in Layer    Details aufklappen; Esc zurück; kein Image-Abruf\n",
    ),
    (
        "F6 / :exec-sessions Switch retained shells (navigation mode only)\n",
        "F6 / :exec-sessions Zwischen offenen Shells wechseln (nur Navigation)\n",
    ),
    (
        "F3 in Logs        JSON/source/severity/time filters on retained entries\n",
        "F3 in Logs        JSON-/Quellen-/Schweregrad-/Zeitfilter für Einträge\n",
    ),
    (
        ":export-logs      Explicit new private JSONL file; may contain secrets\n",
        ":export-logs      Neue private JSONL-Datei; kann Geheimnisse enthalten\n",
    ),
    (
        "gg / G            First / last; G resumes log following\n",
        "gg / G            Anfang / Ende; G zurück zur Live-Ansicht\n",
    ),
    (
        "Ctrl-u / Ctrl-d   Half-page up / down\n",
        "Strg-u / Strg-d   Halbe Seite hoch / runter\n",
    ),
    (
        "\nLogs (focus the log pane with Tab)\n",
        "\nLogs (Log-Bereich mit Tab fokussieren)\n",
    ),
    (
        "w                 Toggle wrapping on/off immediately\n",
        "w                 Umbruch sofort an/aus\n",
    ),
    (
        "h / l             Horizontal scroll when wrapping is off\n",
        "h / l             Horizontal scrollen, wenn der Umbruch aus ist\n",
    ),
    (
        "/                 Search; Enter applies, Esc cancels input\n",
        "/                 Suchen; Enter wendet an, Esc bricht die Eingabe ab\n",
    ),
    (
        "n / N             Next / previous highlighted occurrence\n",
        "n / N             Nächster / vorheriger hervorgehobener Treffer\n",
    ),
    (
        "f                 Toggle follow; scrolling pauses it\n",
        "f                 Live-Ansicht an/aus; Scrollen pausiert sie\n",
    ),
    (
        "Mouse             Click selects, click again opens; right-click back; wheel scrolls; drag log scrollbar\n",
        "Maus              Klick wählt, 2. Klick öffnet; rechts zurück; Rad scrollt\n",
    ),
    (
        "                  Scrollbar + Rows readout track wrapped display lines\n",
        "                  Scrollbar + Zeilenanzeige folgen umbrochenen Zeilen\n",
    ),
    (
        "Esc               Clear highlights without hiding log context\n",
        "Esc               Hervorhebungen löschen, Log-Kontext bleibt\n",
    ),
    (
        "\nSecurity (optional module)\n",
        "\nSicherheit (optionales Modul)\n",
    ),
    (
        "7 / :security     Open the selected image's Security view\n",
        "7 / :security     Sicherheitsansicht des ausgewählten Images öffnen\n",
    ),
    (
        "Images m/M        Check image/all for :scan-images; 7 opens Security\n",
        "Images m/M        Image/alle für :scan-images markieren; 7 Sicherheit\n",
    ),
    (
        "Security Enter    Open advisory details; z enlarge; :cancel-scan stops queue\n",
        "Sicherheit Enter  Advisory öffnen; z vergrößert; :cancel-scan stoppt\n",
    ),
    ("\nActions\n", "\nAktionen\n"),
    (
        "e / :exec         Embedded shell in the right pane\n",
        "e / :exec         Eingebettete Shell im rechten Bereich\n",
    ),
    (
        "                  Services with replicas open a container picker\n",
        "                  Services mit Replikaten öffnen eine Container-Auswahl\n",
    ),
    (
        "4 then Enter      Focus Exec input; start if no session exists\n",
        "4 dann Enter      Exec-Eingabe fokussieren; startet ohne Sitzung\n",
    ),
    (
        "Ctrl-]            Return from shell input to Tha navigation\n",
        "Strg-]            Von der Shell-Eingabe zurück zur Tha-Navigation\n",
    ),
    (
        "                  Esc, Ctrl-C, arrows, numbers go to the shell\n",
        "                  Esc, Strg-C, Pfeile, Zahlen gehen an die Shell\n",
    ),
    (
        "exit              End shell; output stays in the Exec view\n",
        "exit              Shell beenden; Ausgabe bleibt in der Exec-Ansicht\n",
    ),
    (
        ":close-exec       Close session, not the container\n",
        ":close-exec       Sitzung schließen, nicht den Container\n",
    ),
    (
        ":                 Command palette; Tab completes a command\n",
        ":                 Befehlspalette; Tab vervollständigt einen Befehl\n",
    ),
    (
        ":all / :back      All runtimes / previous workspace\n",
        ":all / :back      Alle Runtimes / vorheriger Workspace\n",
    ),
    (
        ":wrap             Toggle wrapping through the command palette\n",
        ":wrap             Umbruch über die Befehlspalette umschalten\n",
    ),
    (
        "? / F1 / :help    Open this cheatsheet\n",
        "? / F1 / :help    Diese Übersicht öffnen\n",
    ),
    (
        "Ctrl-c            Quit Tha in navigation; inside Exec send interrupt\n",
        "Strg-c            Tha in der Navigation beenden; in Exec Interrupt senden\n",
    ),
    (
        "\nPicker: j/k select, Enter chooses, Esc cancels.\n",
        "\nAuswahl: j/k wählen, Enter übernimmt, Esc bricht ab.\n",
    ),
    (
        ", / :settings     Theme, language, keyboard layout, mouse, log wrapping\n",
        ", / :settings     Design, Sprache, Tastaturlayout, Maus, Log-Umbruch\n",
    ),
    (
        "The default theme inherits your terminal's ANSI palette.\n",
        "Das Standard-Design übernimmt die ANSI-Palette des Terminals.\n",
    ),
    (
        "Green circle: running/healthy; yellow: starting/paused/restarting.\n",
        "Grüner Kreis: läuft/healthy; gelb: startet/pausiert/Neustart.\n",
    ),
    (
        "Red circle: unhealthy/failed; neutral: completed/stopped/unknown.\n",
        "Roter Kreis: unhealthy/Fehler; neutral: beendet/gestoppt/unbekannt.\n",
    ),
    ("All", "Alle"),
    ("Stopped-only", "Nur gestoppt"),
    ("Unreferenced", "Ohne Referenz"),
    ("Untagged", "Ohne Tag"),
    ("Managed", "Verwaltet"),
    ("Usage ↓", "Nutzung ↓"),
    ("Name ↑", "Name ↑"),
    ("Size ↓", "Größe ↓"),
    ("Disabled key", "Deaktivierte Taste"),
    ("Help", "Hilfe"),
    ("Views", "Ansichten"),
    ("Commands", "Befehle"),
    (
        "Back / quit from home",
        "Zurück / auf der Startseite beenden",
    ),
    (
        "Cancel / restore / back",
        "Abbrechen / wiederherstellen / zurück",
    ),
    ("Move / scroll down", "Nach unten bewegen / scrollen"),
    ("Move / scroll up", "Nach oben bewegen / scrollen"),
    ("Page down", "Seite runter"),
    ("Page up", "Seite hoch"),
    (
        "First (gg in tree and inventories)",
        "Anfang (gg in Baum und Listen)",
    ),
    (
        "Last / resume log following",
        "Ende / zurück zur Live-Ansicht",
    ),
    ("Open / inspect", "Öffnen / ansehen"),
    ("Next pane", "Nächster Bereich"),
    ("Previous pane", "Vorheriger Bereich"),
    ("Images / findings pane", "Bereich Images / Befunde"),
    ("Previous overview", "Vorherige Übersicht"),
    ("Next overview", "Nächste Übersicht"),
    ("Containers view", "Ansicht Container"),
    ("Images view", "Ansicht Images"),
    ("Volumes view", "Ansicht Volumes"),
    ("Security view", "Ansicht Sicherheit"),
    ("Workload overview", "Workload-Übersicht"),
    ("Logs", "Logs"),
    (
        "Exec tab (Enter resumes input)",
        "Exec-Tab (Enter setzt Eingabe fort)",
    ),
    ("Container metrics", "Container-Metriken"),
    ("Image layers (read-only)", "Image-Layer (nur lesend)"),
    (
        "Selected image security",
        "Sicherheit des ausgewählten Images",
    ),
    (
        "Container events timeline",
        "Zeitleiste der Container-Ereignisse",
    ),
    ("Search", "Suchen"),
    (
        "Collapse / parent; previous tab; scroll unwrapped logs",
        "Einklappen / Ebene hoch; vorheriger Tab; Logs seitlich scrollen",
    ),
    (
        "Expand / child; next tab; scroll unwrapped logs",
        "Ausklappen / Ebene tiefer; nächster Tab; Logs seitlich scrollen",
    ),
    ("Previous detail tab", "Vorheriger Detail-Tab"),
    ("Next detail tab", "Nächster Detail-Tab"),
    ("Previous runtime", "Vorherige Runtime"),
    ("Next runtime", "Nächste Runtime"),
    ("Toggle explorer branch", "Explorer-Zweig umschalten"),
    ("Close Performance", "Performance schließen"),
    ("Half-page down", "Halbe Seite runter"),
    ("Half-page up", "Halbe Seite hoch"),
    ("Next match", "Nächster Treffer"),
    ("Previous match", "Vorheriger Treffer"),
    ("Previous workspace", "Vorheriger Workspace"),
    ("All runtimes", "Alle Runtimes"),
    ("Open container shell", "Container-Shell öffnen"),
    (
        "Contextual actions and disabled reasons",
        "Kontextbezogene Aktionen und Gründe für deaktivierte",
    ),
    (
        "Start (confirmation required)",
        "Starten (Bestätigung nötig)",
    ),
    (
        "Stop (confirmation required)",
        "Stoppen (Bestätigung nötig)",
    ),
    (
        "Restart (confirmation required)",
        "Neu starten (Bestätigung nötig)",
    ),
    (
        "Delete stopped containers (type name to confirm)",
        "Gestoppte Container löschen (zum Bestätigen Name eingeben)",
    ),
    ("Performance", "Performance"),
    (
        "Runtime / container Performance scope",
        "Performance-Bereich Runtime / Container",
    ),
    ("Sort contexts", "Kontexte sortieren"),
    (
        "Expand / restore focused view",
        "Fokussierte Ansicht vergrößern / wiederherstellen",
    ),
    ("Follow logs", "Logs live ansehen"),
    (
        "Export retained logs (may contain secrets)",
        "Gespeicherte Logs exportieren (kann Geheimnisse enthalten)",
    ),
    (
        "Reload logs now (streams also reconnect automatically)",
        "Logs jetzt neu laden (Streams verbinden sich auch automatisch neu)",
    ),
    (
        "Load logs from a start time",
        "Logs ab einer Startzeit laden",
    ),
    (
        "Compose pull, then up -d (confirmation required)",
        "Compose pull, dann up -d (Bestätigung nötig)",
    ),
    (
        "Compose pull (confirmation required)",
        "Compose pull (Bestätigung nötig)",
    ),
    (
        "Compose up -d (confirmation required)",
        "Compose up -d (Bestätigung nötig)",
    ),
    (
        "Compose force recreate (confirmation required)",
        "Compose erzwungen neu erstellen (Bestätigung nötig)",
    ),
    (
        "Compose down, keeping volumes (type name to confirm)",
        "Compose down, Volumes bleiben (zum Bestätigen Name eingeben)",
    ),
    ("Last Compose output", "Letzte Compose-Ausgabe"),
    ("Pretty-print JSON logs", "JSON-Logs formatieren"),
    ("Retained shell sessions", "Offene Shell-Sitzungen"),
    (
        "Close shell session, not container",
        "Shell-Sitzung schließen, nicht den Container",
    ),
    ("Shorter metrics time range", "Kürzerer Metrik-Zeitraum"),
    ("Longer metrics time range", "Längerer Metrik-Zeitraum"),
    (
        "Pan / inspect earlier metrics",
        "Verschieben / frühere Metriken ansehen",
    ),
    (
        "Pan / inspect later metrics",
        "Verschieben / spätere Metriken ansehen",
    ),
    ("Toggle metrics cursor", "Metrik-Cursor umschalten"),
    ("Live metrics", "Live-Metriken"),
    (
        "Mark unused resource for cleanup",
        "Ungenutzte Ressource zur Bereinigung markieren",
    ),
    (
        "Mark visible unused resources",
        "Sichtbare ungenutzte Ressourcen markieren",
    ),
    (
        "Clear resource and scan marks",
        "Ressourcen- und Scan-Markierungen löschen",
    ),
    (
        "Delete unused resources (confirmation required)",
        "Ungenutzte Ressourcen löschen (Bestätigung nötig)",
    ),
    (
        "Prune all unreferenced resources (confirmation required)",
        "Alle ungenutzten Ressourcen bereinigen (Bestätigung nötig)",
    ),
    ("Filter inventory", "Inventar filtern"),
    ("Sort inventory", "Inventar sortieren"),
    ("Refresh all engines", "Alle Engines aktualisieren"),
    (
        "Compare visible images with their registries",
        "Sichtbare Images mit ihren Registries vergleichen",
    ),
    (
        "Pull the selected image's tags",
        "Tags des ausgewählten Images abrufen",
    ),
    ("Check image for scanning", "Image für Scan markieren"),
    (
        "Check all visible images for scanning",
        "Alle sichtbaren Images für Scan markieren",
    ),
    ("Clear scan checks", "Scan-Markierungen löschen"),
    (
        "Scan checked or selected images",
        "Markierte oder ausgewählte Images scannen",
    ),
    ("Scan checked images", "Markierte Images scannen"),
    (
        "Cancel security queue",
        "Sicherheits-Warteschlange abbrechen",
    ),
    (
        "Update scanner database and rescan",
        "Scanner-Datenbank aktualisieren und neu scannen",
    ),
    (
        "Retry image layers lookup",
        "Abfrage der Image-Layer wiederholen",
    ),
    (
        "Background lifecycle progress and results",
        "Fortschritt und Ergebnisse von Container-Aktionen",
    ),
    ("Last cleanup results", "Letzte Bereinigungsergebnisse"),
    (
        "Settings: theme, language, keyboard layout, mouse",
        "Einstellungen: Design, Sprache, Tastaturlayout, Maus",
    ),
    ("Quit Tha", "Tha beenden"),
    ("failed exit", "mit Fehler beendet"),
    ("stopping", "stoppt"),
    ("removing", "wird entfernt"),
    ("Ctrl-]", "Strg-]"),
    ("1 minute", "1 Minute"),
    ("3 minutes", "3 Minuten"),
    ("5 minutes", "5 Minuten"),
    ("15 minutes", "15 Minuten"),
    ("30 minutes", "30 Minuten"),
    ("1 hour", "1 Stunde"),
    ("3 hours", "3 Stunden"),
    ("6 hours", "6 Stunden"),
    ("12 hours", "12 Stunden"),
    ("24 hours", "24 Stunden"),
    ("completed", "abgeschlossen"),
    ("healthy", "healthy"),
    ("alias", "Alias"),
    ("connecting", "verbindet"),
    ("connected", "verbunden"),
    ("German (QWERTZ)", "Deutsch (QWERTZ)"),
    ("Standard keys", "Standardtasten"),
    (
        "Adds - / + for the metrics range and F12 to leave a shell",
        "Ergänzt - / + für den Metrik-Zeitraum und F12 zum Verlassen einer Shell",
    ),
    ("Terminal colors", "Terminalfarben"),
    ("exited", "beendet"),
    ("out of memory", "OOM-Kill"),
    ("health", "Health"),
    ("signalled", "Signal"),
    ("started", "gestartet"),
    ("restarted", "neu gestartet"),
    ("created", "erstellt"),
    ("removed", "entfernt"),
    ("errors", "Fehler"),
    ("failed", "fehlgeschlagen"),
    ("OOM killed", "OOM-Kill"),
    ("paused", "pausiert"),
    ("restarting", "Neustart"),
    ("running", "läuft"),
    ("some stopped", "teils gestoppt"),
    ("starting", "startet"),
    ("stopped/empty", "gestoppt/leer"),
    ("unhealthy", "unhealthy"),
    ("warning", "Warnung"),
    ("workload issues", "Workload-Probleme"),
    ("stopped-only", "nur gestoppt"),
    ("unreferenced", "ohne Referenz"),
    ("unused", "ungenutzt"),
    (
        " Filter retained logs · Advanced JSON ",
        " Gespeicherte Logs filtern · Erweitert (JSON) ",
    ),
    (
        " Filter retained logs · Text ",
        " Gespeicherte Logs filtern · Text ",
    ),
    ("Advanced JSON log filter", "Erweiterter JSON-Logfilter"),
    (
        "Clear text and advanced log filters",
        "Text- und erweiterte Logfilter löschen",
    ),
    (
        "Clear text and advanced log filters without changing search",
        "Text- und erweiterte Logfilter löschen, ohne die Suche zu ändern",
    ),
    (
        "Edit retained-log JSON/source/severity/time filters",
        "JSON-/Quellen-/Schweregrad-/Zeitfilter für gespeicherte Logs bearbeiten",
    ),
    (
        "Enter Apply · Esc Cancel · Ctrl-u Clear · Tab Advanced JSON",
        "Enter Anwenden · Esc Abbrechen · Strg-u Leeren · Tab Erweitert (JSON)",
    ),
    (
        "Enter Apply · Esc Cancel · Ctrl-u Clear · Tab Text; paste appends JSON",
        "Enter Anwenden · Esc Abbrechen · Strg-u Leeren · Tab Text; Einfügen hängt JSON an",
    ),
    (
        "Filter retained logs by text",
        "Gespeicherte Logs nach Text filtern",
    ),
    (
        "Hide retained logs unless the message has every word (F3)",
        "Nur Logs zeigen, deren Nachricht jedes Wort enthält (F3)",
    ),
    (
        "No retained logs match this filter. F3 edits; :clear-log-filter resets.",
        "Keine gespeicherten Logs passen zu diesem Filter. F3 bearbeitet; :clear-log-filter setzt zurück.",
    ),
    (
        "Show entries whose message contains every word, in any order. backend1 500 needs both.\nCase-insensitive literal text: no quotes, operators or regex. Source names do not count.\nEmpty text removes only this constraint. Advanced filters still apply on top.\nHidden entries stay retained; no older logs are fetched. / highlights without hiding.\nTab switches to Advanced JSON for source, severity, fields and time.",
        "Zeigt Einträge, deren Nachricht jedes Wort enthält, in beliebiger Reihenfolge. backend1 500 braucht beide.\nWörtlicher Text ohne Groß-/Kleinschreibung: keine Anführungszeichen, Operatoren oder Regex. Quellnamen zählen nicht.\nLeerer Text entfernt nur diese Bedingung. Erweiterte Filter gelten weiterhin zusätzlich.\nAusgeblendete Einträge bleiben gespeichert; ältere Logs werden nicht geladen. / hebt hervor, ohne auszublenden.\nTab wechselt zu Erweitert (JSON) für Quelle, Schweregrad, Felder und Zeit.",
    ),
    (
        "Showing entries whose message contains every term. Retained logs are kept; F3 edits, :clear-log-filter resets.",
        "Es werden Einträge gezeigt, deren Nachricht jeden Begriff enthält. Gespeicherte Logs bleiben erhalten; F3 bearbeitet, :clear-log-filter setzt zurück.",
    ),
    (
        "Text filter cleared; advanced filters are unchanged.",
        "Textfilter gelöscht; erweiterte Filter bleiben unverändert.",
    ),
    ("advanced", "erweitert"),
    (
        "{matched} matching / {a0} retained entries in the current scope. Filter: {filter}.\nLogs may contain SECRETS. Choose an explicit destination filename.\nNew JSONL file only; mode 0600; existing files/symlinks refused.\nNo history fetch, no automatic redaction, no silent truncation.\nSnapshot limit: 16 MiB; destination directory must already exist.",
        "{matched} passende / {a0} gespeicherte Einträge im aktuellen Bereich. Filter: {filter}.\nLogs können GEHEIMNISSE enthalten. Einen expliziten Zieldateinamen wählen.\nNur neue JSONL-Datei; Modus 0600; vorhandene Dateien/Symlinks werden abgelehnt.\nKein Nachladen des Verlaufs, kein automatisches Schwärzen, kein stilles Kürzen.\nSnapshot-Limit: 16 MiB; das Zielverzeichnis muss bereits existieren.",
    ),
    ("{terms} + advanced", "{terms} + erweitert"),
    (
        "    Also attached: {names}",
        "    Ebenfalls verbunden: {names}",
    ),
    (
        "    IPv4 {ipv4} · IPv6 {ipv6}",
        "    IPv4 {ipv4} · IPv6 {ipv6}",
    ),
    (
        "    Reported aliases: {aliases} (resolution not tested)",
        "    Gemeldete Aliase: {aliases} (Auflösung nicht getestet)",
    ),
    (
        "    Reported aliases: {value}",
        "    Gemeldete Aliase: {value}",
    ),
    (
        "    That container is not running, so its binding is configuration, not active.",
        "    Dieser Container läuft nicht; seine Bindung ist also Konfiguration und nicht aktiv.",
    ),
    (
        "    {hidden} attachments outside this project (hidden)",
        "    {hidden} Verbindungen außerhalb dieses Projekts (ausgeblendet)",
    ),
    ("    {more} more attached", "    {more} weitere verbunden"),
    (
        "    {omitted} more aliases not shown",
        "    {omitted} weitere Aliase nicht angezeigt",
    ),
    (
        "    … output truncated at 4 KiB",
        "    … Ausgabe bei 4 KiB abgeschnitten",
    ),
    (
        "  Addresses are on the runtime host. They are not verified from this workstation.",
        "  Die Adressen liegen auf dem Runtime-Host und wurden von diesem Rechner aus nicht geprüft.",
    ),
    (
        "  An OOM event at {oom} came before the failure at {failure}. The order alone does not prove a cause.",
        "  Ein OOM-Ereignis um {oom} lag vor dem Fehler um {failure}. Die Reihenfolge allein beweist keine Ursache.",
    ),
    (
        "  Both report a network named {name}, but with different IDs. It is not the same network.",
        "  Beide melden ein Netzwerk namens {name}, aber mit unterschiedlichen IDs. Es ist nicht dasselbe Netzwerk.",
    ),
    (
        "  Comparison incomplete: a network ID is missing, and names do not prove identity.",
        "  Vergleich unvollständig: Eine Netzwerk-ID fehlt, und Namen belegen keine Identität.",
    ),
    (
        "  Comparison incomplete: host, none or container-sharing mode is not described by network membership.",
        "  Vergleich unvollständig: Die Modi host, none und Container-Sharing lassen sich nicht über die Netzwerkmitgliedschaft beschreiben.",
    ),
    (
        "  Comparison incomplete: network details are missing for at least one container.",
        "  Vergleich unvollständig: Für mindestens einen Container fehlen Netzwerkdetails.",
    ),
    (
        "  Comparison incomplete: the two containers are not known to be on the same runtime.",
        "  Vergleich unvollständig: Es ist nicht bekannt, ob beide Container auf derselben Runtime liegen.",
    ),
    (
        "  Coverage: last 24 hours, saved by the service",
        "  Abdeckung: letzte 24 Stunden, vom Dienst gespeichert",
    ),
    (
        "  Coverage: only what the engine retained and Tha saw while open",
        "  Abdeckung: nur was die Engine vorhält und Tha während der Laufzeit gesehen hat",
    ),
    ("  Error      {error}", "  Fehler     {error}"),
    (
        "  Host processes are not checked, and no bind failure is implied.",
        "  Host-Prozesse werden nicht geprüft, und ein Bind-Fehler ist damit nicht gemeint.",
    ),
    ("  Inventory  {status}", "  Inventar   {status}"),
    ("  Last exit code {code}", "  Letzter Exit-Code {code}"),
    (
        "  Last exit code {code} at {at} (history, not necessarily the current cause)",
        "  Letzter Exit-Code {code} um {at} (Verlauf, nicht unbedingt die aktuelle Ursache)",
    ),
    (
        "  Last exit code {code} · no exit time reported",
        "  Letzter Exit-Code {code} · keine Exit-Zeit gemeldet",
    ),
    ("  Last started {at}", "  Zuletzt gestartet {at}"),
    (
        "  Membership is unknown.",
        "  Mitgliedschaft ist unbekannt.",
    ),
    ("  Mode {mode}", "  Modus {mode}"),
    (
        "  No attempts reported yet.",
        "  Noch keine Versuche gemeldet.",
    ),
    (
        "  No current failure is reported. History below may still be useful.",
        "  Aktuell wird kein Fehler gemeldet. Der Verlauf unten kann trotzdem hilfreich sein.",
    ),
    (
        "  No events available for this container ID.",
        "  Für diese Container-ID sind keine Ereignisse verfügbar.",
    ),
    (
        "  No health check is configured, so none has passed or failed.",
        "  Es ist kein Healthcheck konfiguriert, also ist auch keiner bestanden oder fehlgeschlagen.",
    ),
    (
        "  No logs are retained for exactly this container yet. Opening this view does not load them; l does.",
        "  Für genau diesen Container sind noch keine Logs gespeichert. Diese Ansicht lädt sie nicht; l schon.",
    ),
    (
        "  No network attachments reported.",
        "  Keine Netzwerkverbindungen gemeldet.",
    ),
    (
        "  No peer chosen. Choose a container on this runtime to compare network identities.",
        "  Kein Vergleichscontainer gewählt. Einen Container auf dieser Runtime wählen, um Netzwerk-IDs zu vergleichen.",
    ),
    ("  No ports reported.", "  Keine Ports gemeldet."),
    (
        "  No retained log has a timestamp near the failure at {at}.",
        "  Kein gespeichertes Log hat einen Zeitstempel nahe dem Fehler um {at}.",
    ),
    (
        "  No shared network is reported. This does not rule out published-port, host-network, routed or external paths.",
        "  Es wird kein gemeinsames Netzwerk gemeldet. Wege über veröffentlichte Ports, das Host-Netzwerk, Routing oder extern sind damit nicht ausgeschlossen.",
    ),
    (
        "  Nothing in the available evidence suggests one.",
        "  Die verfügbaren Hinweise legen keine nahe.",
    ),
    ("  OOM flag not set", "  OOM-Flag nicht gesetzt"),
    (
        "  OOM flag set for the run that ended {at} (history; not a proven cause)",
        "  OOM-Flag gesetzt für den Lauf, der um {at} endete (Verlauf; keine bewiesene Ursache)",
    ),
    (
        "  OOM flag set; time unavailable (not a proven cause)",
        "  OOM-Flag gesetzt; Zeit nicht verfügbar (keine bewiesene Ursache)",
    ),
    ("  OOM flag {value}", "  OOM-Flag {value}"),
    (
        "  Peer {name} · ID {id}",
        "  Vergleich mit {name} · ID {id}",
    ),
    (
        "  Possible overlap, not confirmed: {other} also reports host port {port}/{protocol}.",
        "  Mögliche Überschneidung, nicht bestätigt: {other} meldet ebenfalls Host-Port {port}/{protocol}.",
    ),
    (
        "  Possible restart loop: {count} timed exits within 10 minutes in the available events.",
        "  Mögliche Neustartschleife: {count} Exits mit Zeitstempel innerhalb von 10 Minuten in den verfügbaren Ereignissen.",
    ),
    (
        "  Recent available logs. No failure time is known, so they are not tied to a failure.",
        "  Neueste verfügbare Logs. Es ist keine Fehlerzeit bekannt, daher gehören sie nicht zwingend zu einem Fehler.",
    ),
    ("  Restart count {count}", "  Neustarts {count}"),
    (
        "  Restart count {count} (a total; it does not show a restart loop)",
        "  Neustarts {count} (Gesamtzahl; zeigt keine Neustartschleife)",
    ),
    (
        "  Shared network: {networks}. This is configuration; connectivity has not been tested.",
        "  Gemeinsames Netzwerk: {networks}. Das ist Konfiguration; die Verbindung wurde nicht getestet.",
    ),
    ("  State      {state}", "  Zustand    {state}"),
    (
        "  Status {status} (inventory). Check details are unknown.",
        "  Status {status} (Inventar). Details zum Check sind unbekannt.",
    ),
    (
        "  Status {status} · failing streak {streak}",
        "  Status {status} · Fehlerserie {streak}",
    ),
    (
        "  This container is not running, so these bindings are configuration, not active.",
        "  Dieser Container läuft nicht; diese Bindungen sind also Konfiguration und nicht aktiv.",
    ),
    (
        "  Timestamped within 60 s before or 10 s after the failure at {at}.",
        "  Zeitstempel bis 60 s vor oder 10 s nach dem Fehler um {at}.",
    ),
    (
        "  Uncertain overlap: {other} also reports host port {port}/{protocol}. It depends on the address family or an unreported address.",
        "  Unsichere Überschneidung: {other} meldet ebenfalls Host-Port {port}/{protocol}. Das hängt von der Adressfamilie oder einer nicht gemeldeten Adresse ab.",
    ),
    (
        "  Why?       :diagnose shows the evidence (read-only)",
        "  Warum?     :diagnose zeigt die Hinweise (nur lesend)",
    ),
    (
        "  container: shares the network stack of {other}.",
        "  container: nutzt den Netzwerk-Stack von {other} mit.",
    ),
    (
        "  host: uses the runtime host's network stack, so it has no ordinary network membership or container address.",
        "  host: nutzt den Netzwerk-Stack des Runtime-Hosts und hat daher keine normale Netzwerkmitgliedschaft oder Container-Adresse.",
    ),
    (
        "  none: no network is configured for this container.",
        "  none: Für diesen Container ist kein Netzwerk konfiguriert.",
    ),
    ("  {at} · exit {code}", "  {at} · Exit {code}"),
    (
        "  {hidden} of these are hidden in Logs by the active filter ({filter}).",
        "  {hidden} davon blendet der aktive Filter ({filter}) in den Logs aus.",
    ),
    (
        "  {hidden} possible port overlaps with containers outside this project (hidden)",
        "  {hidden} mögliche Port-Überschneidungen mit Containern außerhalb dieses Projekts (ausgeblendet)",
    ),
    ("  {mode}", "  {mode}"),
    (
        "  {network} · ID unknown · addresses unavailable",
        "  {network} · ID unbekannt · Adressen nicht verfügbar",
    ),
    ("  {network} · ID {id}", "  {network} · ID {id}"),
    (
        "  {omitted} more networks not shown",
        "  {omitted} weitere Netzwerke nicht angezeigt",
    ),
    (
        "  {omitted} older attempts not shown",
        "  {omitted} ältere Versuche nicht angezeigt",
    ),
    ("  {port} (reported)", "  {port} (gemeldet)"),
    (" Troubleshoot network ", " Netzwerk prüfen "),
    (" Why is this unhealthy? ", " Warum ist das unhealthy? "),
    (
        " j/k Scroll · r Refresh · l Logs · e Events · m Metrics · Esc Close ",
        " j/k Scrollen · r Aktualisieren · l Logs · e Ereignisse · m Metriken · Esc Schließen ",
    ),
    (
        " j/k Scroll · r Refresh · p Peer · n Networks · Esc Close ",
        " j/k Scrollen · r Aktualisieren · p Vergleich · n Netzwerke · Esc Schließen ",
    ),
    (
        ":network-diagnose shows addresses, ports and a peer comparison (no probes)",
        ":network-diagnose zeigt Adressen, Ports und einen Containervergleich (ohne Tests)",
    ),
    (
        "Compare with a container on this runtime",
        "Mit einem Container auf dieser Runtime vergleichen",
    ),
    (
        "Connection unavailable; detailed inspection was not attempted",
        "Verbindung nicht verfügbar; die Detailabfrage wurde nicht versucht",
    ),
    (
        "Container inspection timed out after 5s",
        "Container-Abfrage nach 5 s abgebrochen",
    ),
    ("Current state", "Aktueller Zustand"),
    (
        "Detailed inspection unavailable: {error}",
        "Detailabfrage nicht verfügbar: {error}",
    ),
    (
        "Evidence stays on this machine and is not saved or exported by this view.",
        "Die Hinweise bleiben auf diesem Rechner; diese Ansicht speichert und exportiert nichts.",
    ),
    ("Exit, OOM and restarts", "Exit, OOM und Neustarts"),
    (
        "Exposed and published ports",
        "Freigegebene und veröffentlichte Ports",
    ),
    ("Health checks", "Healthchecks"),
    (
        "Loading from the runtime…",
        "Wird von der Runtime geladen …",
    ),
    (
        "Logs keep the active filter ({filter}), which may hide related entries. :clear-log-filter shows them.",
        "Die Logs behalten den aktiven Filter ({filter}), der zugehörige Einträge ausblenden kann. :clear-log-filter zeigt sie.",
    ),
    (
        "Metrics show only samples collected so far; earlier history may not exist.",
        "Metriken zeigen nur bisher erfasste Messwerte; ein früherer Verlauf existiert eventuell nicht.",
    ),
    ("Network mode", "Netzwerkmodus"),
    ("Networks and addresses", "Netzwerke und Adressen"),
    (
        "No other container is visible on this runtime",
        "Auf dieser Runtime ist kein weiterer Container sichtbar",
    ),
    ("No peer", "Kein Vergleich"),
    ("Observed {at}", "Abgefragt {at}"),
    (
        "Passive network evidence and peer comparison; sends no probes",
        "Passive Netzwerkhinweise und Containervergleich; sendet keine Tests",
    ),
    (
        "Passive: reported configuration only. Nothing is resolved, pinged or probed, so connectivity is not tested.",
        "Passiv: nur gemeldete Konfiguration. Nichts wird aufgelöst, angepingt oder getestet, die Verbindung bleibt also ungeprüft.",
    ),
    (
        "Peer comparison (p chooses a peer)",
        "Containervergleich (p wählt einen Container)",
    ),
    (
        "Peer comparison needs detailed inspection, which this backend does not have",
        "Der Containervergleich braucht die Detailabfrage, die dieses Backend nicht bietet",
    ),
    (
        "Possible explanations (not proven)",
        "Mögliche Erklärungen (nicht bewiesen)",
    ),
    (
        "Read-only: no health check is run and nothing is changed. Reported evidence, not a verdict.",
        "Nur lesend: Es wird kein Healthcheck ausgeführt und nichts geändert. Gemeldete Hinweise, kein Urteil.",
    ),
    (
        "Related events ({shown} of {total} · e opens Events)",
        "Zugehörige Ereignisse ({shown} von {total} · e öffnet Ereignisse)",
    ),
    (
        "Related logs (l opens Logs)",
        "Zugehörige Logs (l öffnet Logs)",
    ),
    (
        "Runtime answered for a different container; result discarded",
        "Die Runtime hat für einen anderen Container geantwortet; Ergebnis verworfen",
    ),
    (
        "Runtime {runtime} · Container {name} · ID {id}",
        "Runtime {runtime} · Container {name} · ID {id}",
    ),
    (
        "Select a single container; diagnostics do not cover groups",
        "Einen einzelnen Container auswählen; die Diagnose gilt nicht für Gruppen",
    ),
    (
        "Stale: the endpoint now reports a different runtime. This evidence is about the previous one.",
        "Veraltet: Der Endpunkt meldet jetzt eine andere Runtime. Diese Hinweise betreffen die vorherige.",
    ),
    (
        "Stale: the engine is offline, so this may no longer be true.",
        "Veraltet: Die Engine ist offline, das stimmt also eventuell nicht mehr.",
    ),
    (
        "Stale: this container ID is gone. A new container with the same name has none of this evidence.",
        "Veraltet: Diese Container-ID gibt es nicht mehr. Ein neuer Container gleichen Namens hat nichts mit diesen Hinweisen zu tun.",
    ),
    (
        "This backend has no detailed inspection; only inventory facts are shown.",
        "Dieses Backend bietet keine Detailabfrage; es werden nur Inventardaten angezeigt.",
    ),
    (
        "This container is no longer the selection; close and review the current one",
        "Dieser Container ist nicht mehr ausgewählt; schließen und den aktuellen prüfen",
    ),
    (
        "This container no longer exists here; close and select the current one",
        "Diesen Container gibt es hier nicht mehr; schließen und den aktuellen auswählen",
    ),
    (
        "Troubleshoot network (read-only, no probes)",
        "Netzwerk prüfen (nur lesend, ohne Tests)",
    ),
    (
        "Troubleshoot network · read-only",
        "Netzwerk prüfen · nur lesend",
    ),
    (
        "Why is this unhealthy? (read-only evidence)",
        "Warum ist das unhealthy? (nur lesende Hinweise)",
    ),
    (
        "Why is this unhealthy? Read-only evidence for one container",
        "Warum ist das unhealthy? Nur lesende Hinweise zu einem Container",
    ),
    (
        "Why is this unhealthy? · read-only",
        "Warum ist das unhealthy? · nur lesend",
    ),
    (
        "a container outside this project",
        "einem Container außerhalb dieses Projekts",
    ),
    ("time unavailable", "Zeit nicht verfügbar"),
    (
        "  Exit code: none for the current run. It is cleared on start, so look at the events for earlier exits.",
        "  Exit-Code: keiner für den aktuellen Lauf. Er wird beim Start gelöscht; frühere Exits stehen in den Ereignissen.",
    ),
    ("  Previous run ended {at}", "  Vorheriger Lauf endete {at}"),
    (
        "  OOM flag not set for the current run; it is cleared on start",
        "  OOM-Flag für den aktuellen Lauf nicht gesetzt; es wird beim Start gelöscht",
    ),
    ("  Health     {a0}", "  Health     {a0}"),
    (
        "  The check command and schedule are on the Overview tab (1).",
        "  Check-Befehl und Zeitplan stehen im Tab Übersicht (1).",
    ),
    ("  Command    {a0}", "  Befehl     {a0}"),
    (
        "  Failing    {count} checks in a row",
        "  Fehler     {count} Checks in Folge",
    ),
    (
        "  Last check {at} · exit {code}",
        "  Letzter    {at} · Exit {code}",
    ),
    ("  Output     {a0}", "  Ausgabe    {a0}"),
    ("  Entrypoint {a0}", "  Entrypoint {a0}"),
    (
        "  Healthcheck (from the image)",
        "  Healthcheck (aus dem Image)",
    ),
    (
        "  Healthcheck (set on this container, by Compose or run)",
        "  Healthcheck (am Container gesetzt, per Compose oder run)",
    ),
    ("  Healthcheck", "  Healthcheck"),
    (" Add server ", " Server hinzufügen "),
    (
        " All runtimes · Enter a machine or Compose project to focus it",
        " Alle Runtimes · Enter öffnet einen Rechner oder ein Compose-Projekt",
    ),
    (
        " All runtimes › {name}  · Backspace Back",
        " Alle Runtimes › {name}  · Backspace Zurück",
    ),
    (" Edit server ", " Server bearbeiten "),
    (" Remove server ", " Server entfernen "),
    (" Servers ", " Server "),
    (
        " Tab/↑/↓ Move · Enter Select · Ctrl+S Save · Esc Back ",
        " Tab/↑/↓ Wechseln · Enter Auswählen · Strg+S Speichern · Esc Zurück ",
    ),
    (" [not connected]", " [nicht verbunden]"),
    (
        " a Add · e Edit · d Remove · Enter Connect/Disconnect · Esc Close ",
        " a Hinzufügen · e Bearbeiten · d Entfernen · Enter Verbinden/Trennen · Esc Schließen ",
    ),
    (" y Remove · Esc Cancel ", " y Entfernen · Esc Abbrechen "),
    (
        "A file already exists at this path; it was not changed",
        "Unter diesem Pfad gibt es schon eine Datei; sie wurde nicht geändert",
    ),
    (
        "A server with this name already exists",
        "Ein Server mit diesem Namen existiert bereits",
    ),
    (
        "Cannot read {path}; create it with ssh-keygen -y -f KEY > KEY.pub",
        "{path} ist nicht lesbar; erstellen mit ssh-keygen -y -f KEY > KEY.pub",
    ),
    (
        "Cannot run ssh; install OpenSSH and ensure it is on PATH",
        "ssh lässt sich nicht starten; OpenSSH installieren und zum PATH hinzufügen",
    ),
    (
        "Changes last for this session: saving is turned off.",
        "Änderungen gelten nur für diese Sitzung: Speichern ist deaktiviert.",
    ),
    (
        "Choose an SSH key to see the command for the server.",
        "SSH-Schlüssel auswählen, um den Befehl für den Server zu sehen.",
    ),
    (
        "Compose is not available on servers: its files are on the server, not here",
        "Compose ist auf Servern nicht verfügbar: Die Dateien liegen dort, nicht hier",
    ),
    ("Connect", "Verbinden"),
    (
        "Connect or disconnect the selected server",
        "Ausgewählten Server verbinden oder trennen",
    ),
    ("Connect to servers", "Mit Servern verbinden"),
    ("Connected", "Verbunden"),
    (
        "Connected · delete {delete}",
        "Verbunden · Löschen {delete}",
    ),
    (
        "Connecting over SSH…",
        "Verbindung über SSH wird aufgebaut…",
    ),
    ("Connection works.", "Verbindung funktioniert."),
    (
        "Connection works. Engines: {engines}",
        "Verbindung funktioniert. Engines: {engines}",
    ),
    (
        "Copy server setup command",
        "Setup-Befehl für den Server kopieren",
    ),
    (
        "Could not save servers: {error}",
        "Server konnten nicht gespeichert werden: {error}",
    ),
    ("Create SSH key {path}", "SSH-Schlüssel {path} erstellen"),
    (
        "Create new key at this path",
        "Neuen Schlüssel unter diesem Pfad erstellen",
    ),
    (
        "Deleting is turned off on this server",
        "Löschen ist auf diesem Server deaktiviert",
    ),
    (
        "Demo mode: servers are not contacted",
        "Demo-Modus: Server werden nicht kontaktiert",
    ),
    (
        "Discard your changes? y Discard · any other key keeps editing",
        "Änderungen verwerfen? y Verwerfen · jede andere Taste bearbeitet weiter",
    ),
    (
        "Each server runs tha-agent, which only allows start, stop, restart, logs and inventory.",
        "Auf jedem Server läuft tha-agent und erlaubt nur Starten, Stoppen, Neustarten, Logs und Inventar.",
    ),
    (
        "Enter adds, edits or removes servers.",
        "Enter fügt Server hinzu, bearbeitet oder entfernt sie.",
    ),
    (
        "Exec is not available on servers: the agent does not allow running commands",
        "Exec ist auf Servern nicht verfügbar: Der Agent erlaubt keine Befehle",
    ),
    (
        "HOME is not set, so ~/ cannot be expanded",
        "HOME ist nicht gesetzt, ~/ lässt sich also nicht auflösen",
    ),
    ("Host", "Host"),
    (
        "Host key unknown or changed. Use Test connection under Servers.",
        "Host-Schlüssel unbekannt oder geändert. Unter Server die Verbindung testen.",
    ),
    (
        "Host must not start with - or contain spaces, quotes or slashes",
        "Host darf nicht mit - beginnen und keine Leerzeichen, Anführungszeichen oder Schrägstriche enthalten",
    ),
    (
        "Key created. Run the setup command on the server.",
        "Schlüssel erstellt. Jetzt den Setup-Befehl auf dem Server ausführen.",
    ),
    (
        "Key refused. Run the setup command on the server; if the key has a passphrase, load it with ssh-add.",
        "Schlüssel abgelehnt. Setup-Befehl auf dem Server ausführen; einen Schlüssel mit Passphrase mit ssh-add laden.",
    ),
    (
        "Machines have no actions; select a workload inside one",
        "Rechner haben keine Aktionen; einen Workload darin auswählen",
    ),
    ("Manual", "Manuell"),
    ("Needs attention", "Handlung nötig"),
    (
        "No servers yet. Press a to add one.",
        "Noch keine Server. Mit a einen hinzufügen.",
    ),
    ("Not connected", "Nicht verbunden"),
    (
        "Not connected, so no traffic is sent. Enter connects and opens this server.",
        "Nicht verbunden, es fließt also kein Traffic. Enter verbindet und öffnet diesen Server.",
    ),
    ("Offline, retrying", "Offline, neuer Versuch läuft"),
    ("On startup", "Beim Start"),
    ("Port", "Port"),
    (
        "Port must be between 1 and 65535",
        "Port muss zwischen 1 und 65535 liegen",
    ),
    (
        "Pulling images is not available on servers: the agent does not allow it",
        "Images pullen ist auf Servern nicht verfügbar: Der Agent erlaubt es nicht",
    ),
    (
        "Remove {name} from this list and disconnect it?",
        "{name} aus dieser Liste entfernen und trennen?",
    ),
    (
        "Removed {name}. Its key still works on the server until removed from that user's authorized_keys.",
        "{name} entfernt. Der Schlüssel funktioniert auf dem Server weiter, bis er aus den authorized_keys des Benutzers entfernt wird.",
    ),
    (
        "SSH cannot use this key. Check the file and that its mode is 600.",
        "SSH kann diesen Schlüssel nicht verwenden. Datei prüfen und Rechte auf 600 setzen.",
    ),
    (
        "SSH ended without an error message",
        "SSH wurde ohne Fehlermeldung beendet",
    ),
    ("SSH key", "SSH-Schlüssel"),
    ("SSH key      {key}", "SSH-Schlüssel {key}"),
    (
        "SSH key is not a file: {path}",
        "SSH-Schlüssel ist keine Datei: {path}",
    ),
    (
        "SSH key must be an absolute path or start with ~/ or $HOME/",
        "SSH-Schlüssel muss ein absoluter Pfad sein oder mit ~/ oder $HOME/ beginnen",
    ),
    (
        "SSH key not found: {path}",
        "SSH-Schlüssel nicht gefunden: {path}",
    ),
    (
        "SSH key path contains control characters",
        "Pfad zum SSH-Schlüssel enthält Steuerzeichen",
    ),
    ("Save", "Speichern"),
    (
        "Saving reconnects this server. Save again to continue.",
        "Beim Speichern wird dieser Server neu verbunden. Zum Fortfahren erneut speichern.",
    ),
    ("Select a server first", "Zuerst einen Server auswählen"),
    (
        "Sent to the clipboard, if your terminal allows it.",
        "In die Zwischenablage gelegt, sofern das Terminal es erlaubt.",
    ),
    (
        "Server agent problem: {detail}",
        "Problem mit dem Agenten auf dem Server: {detail}",
    ),
    (
        "Server names can use letters, digits, spaces, - _ and .",
        "Servernamen dürfen Buchstaben, Ziffern, Leerzeichen, - _ und . enthalten",
    ),
    ("Servers", "Server"),
    (
        "Servers: add, edit or remove remote servers",
        "Server: Remote-Server hinzufügen, bearbeiten oder entfernen",
    ),
    (
        "Skipped saved server {name}: the name is already in config.toml",
        "Gespeicherter Server {name} übersprungen: Der Name steht schon in der config.toml",
    ),
    (
        "Step 1: run this on the server to install and set up the agent:",
        "Schritt 1: Das hier auf dem Server ausführen, um den Agenten zu installieren und einzurichten:",
    ),
    (
        "Step 2: Test connection confirms the host key. Step 3: Save.",
        "Schritt 2: Verbindung testen bestätigt den Host-Schlüssel. Schritt 3: Speichern.",
    ),
    ("Test connection", "Verbindung testen"),
    ("Test connection to {name}", "Verbindung zu {name} testen"),
    (
        "The key file and the server are not changed.",
        "Die Schlüsseldatei und der Server bleiben unverändert.",
    ),
    (
        "The server did not answer the engine list request",
        "Der Server hat die Anfrage nach den Engines nicht beantwortet",
    ),
    (
        "The server agent exposes no engines",
        "Der Agent auf dem Server gibt keine Engines frei",
    ),
    (
        "The server reported an invalid engine name",
        "Der Server hat einen ungültigen Engine-Namen gemeldet",
    ),
    (
        "The server's engine list is too large",
        "Die Engine-Liste des Servers ist zu groß",
    ),
    ("Local", "Lokal"),
    (
        "This server does not run tha-agent. Run the setup command on it.",
        "Auf diesem Server läuft tha-agent nicht. Den Setup-Befehl dort ausführen.",
    ),
    (
        "This server is in config.toml; edit it there.",
        "Dieser Server steht in der config.toml; dort bearbeiten.",
    ),
    (
        "This server is in config.toml; remove it there.",
        "Dieser Server steht in der config.toml; dort entfernen.",
    ),
    (
        "Timed out connecting over SSH",
        "Zeitüberschreitung beim Verbinden über SSH",
    ),
    (
        "Type the path for the new key first, such as ~/.ssh/tha_prod",
        "Zuerst den Pfad für den neuen Schlüssel eingeben, z. B. ~/.ssh/tha_prod",
    ),
    (
        "Use the Settings value ({mode})",
        "Wert aus den Einstellungen ({mode})",
    ),
    (
        "Used when tha opens. The background service never connects to servers.",
        "Gilt beim Öffnen von tha. Der Hintergrunddienst verbindet sich nie mit Servern.",
    ),
    ("User", "Benutzer"),
    (
        "User must not start with - or contain spaces, quotes, slashes or @",
        "Benutzer darf nicht mit - beginnen und keine Leerzeichen, Anführungszeichen, Schrägstriche oder @ enthalten",
    ),
    (
        "metrics_refresh_seconds must be between 1 and 60",
        "metrics_refresh_seconds muss zwischen 1 und 60 liegen",
    ),
    (
        "{count} engines · delete {delete}",
        "{count} Engines · Löschen {delete}",
    ),
    ("{running}/{total} running", "{running}/{total} läuft"),
    (
        "{status}. Read the output above, then :servers returns to the form.",
        "{status}. Ausgabe oben lesen, :servers führt zurück zum Formular.",
    ),
    ("not connected", "nicht verbunden"),
    (
        "On server {name} ({address})",
        "Auf Server {name} ({address})",
    ),
    (
        "No engines yet. Press . or type :servers to add a server.",
        "Noch keine Engines. Mit . oder :servers einen Server hinzufügen.",
    ),
    (
        "Root key: tha's traffic uses the allowlist, but anyone holding this key has full root access.",
        "Root-Schlüssel: Der Traffic von tha läuft über die Allowlist, aber wer diesen Schlüssel hat, hat vollen Root-Zugriff.",
    ),
    (
        "tha-agent is not installed on this server. Run the setup command on it.",
        "tha-agent ist auf diesem Server nicht installiert. Den Setup-Befehl dort ausführen.",
    ),
    ("<name>", "<Name>"),
    ("<name|address>", "<Name|Adresse>"),
    ("<user>", "<Benutzer>"),
    ("<port>", "<Port>"),
    ("<path>", "<Pfad>"),
    (
        "tha-agent is installed on this server, but not set up. Run the setup command on it.",
        "tha-agent ist auf diesem Server installiert, aber nicht eingerichtet. Den Setup-Befehl dort ausführen.",
    ),
    (" (remote)", " (Remote)"),
    (
        "The connection to the server was lost",
        "Die Verbindung zum Server wurde unterbrochen",
    ),
    (
        "This server runs an older tha-agent. Update it by running the setup command on it.",
        "Auf diesem Server läuft ein älterer tha-agent. Zum Aktualisieren den Setup-Befehl dort ausführen.",
    ),
];
