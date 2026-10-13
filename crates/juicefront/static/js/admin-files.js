
    var PAGE = 50;
    var allFiles = [];
    var displayedCount = 0;
    var currentTotal = 0;
    var searchTimer = null;

    function formatTime(sec) {
        return new Date(sec * 1000).toLocaleString();
    }

    function statusLabel(expiresAt) {
        var left = expiresAt * 1000 - Date.now();
        if (left <= 0) return "Expired";
        var mins = Math.floor(left / 60000);
        var hours = Math.floor(left / 3600000);
        var days = Math.floor(left / 86400000);
        if (mins < 60) return mins + "m";
        if (hours < 24) return hours + "h";
        return days + "d";
    }

    function esc(s) {
        var d = document.createElement("div");
        d.appendChild(document.createTextNode(s));
        return d.innerHTML;
    }

    function buildParams() {
        var q = document.getElementById("search-input").value.trim();
        var sort = document.getElementById("sort-select").value;
        var dir = document.getElementById("dir-select").value;
        return { q: q, sort: sort, dir: dir, offset: 0, limit: PAGE };
    }

    function filterByType(files) {
        var typeFilter = document.getElementById("filter-type").value;
        if (!typeFilter) return files;
        return files.filter(function (f) {
            return f.mime_type.startsWith(typeFilter + "/");
        });
    }

    function tierBadge(f) {
        var st = f.tier_state || (f.hot ? "hot" : (f.tier || "standard"));
        if (st === "hot") return '<span class="hot-badge">HOT</span>';
        if (st === "warming") return '<span class="tier-badge tier-warming">WARMING</span>';
        if (st === "cooling") return '<span class="tier-badge tier-cooling">COOLING</span>';
        return '<span class="tier-standard">' + esc(f.tier || "standard") + "</span>";
    }

    function renderRows(files) {
        var html = "";
        for (var i = 0; i < files.length; i++) {
            var f = files[i];
            var tierState = f.tier_state || (f.hot ? "hot" : "standard");
            var tierAction = (tierState === "hot" || tierState === "warming") ? "cool" : "warm";
            var tierLabel = tierAction === "cool" ? "Cool" : "Warm";
            var expired = f.expires_at * 1000 <= Date.now();
            var rowClass = expired ? ' class="expired"' : '';
            html +=
                '<tr' + rowClass + ' data-file-id="' + esc(f.id) + '">' +
                "<td>" + esc(f.id) + "</td>" +
                "<td>" + esc(f.filename) + "</td>" +
                "<td nowrap>" + formatSize(f.size_bytes) + "</td>" +
                "<td>" + esc(f.mime_type) + "</td>" +
                "<td nowrap>" + formatTime(f.uploaded_at) + "</td>" +
                "<td nowrap>" + formatTime(f.expires_at) + "</td>" +
                "<td nowrap>" + statusLabel(f.expires_at) + "</td>" +
                "<td nowrap title=\"" + String(f.viewers || 0) + " unique viewers, " + String(f.downloads || 0) + " downloads\">" + String(f.views || 0) + "</td>" +
                "<td>" + tierBadge(f) + "</td>" +
                "<td>" + (f.encrypted ? '<span class="enc-badge">ENC</span>' : (f.protected ? '<span class="prot-badge">LOCKED</span>' : '<span class="enc-no">No</span>')) + "</td>" +
                "<td>" +
                    (f.uploader_ip_hash
                        ? '<a href="#" data-ban-trigger data-ban-hash="' + esc(f.uploader_ip_hash) + '">' + esc(f.uploader_ip_hash) + "</a>"
                        : "???") +
                "</td>" +
                "<td nowrap>" +
                    '<a href="' + esc(f.url) + '" target="_blank">View</a> | ' +
                    '<a href="/file/' + encodeURIComponent(f.id) + '/info" target="_blank">Info</a> | ' +
                    '<a href="#" data-tier-toggle data-file-id="' + esc(f.id) + '" data-tier-action="' + tierAction + '">' + tierLabel + '</a> | ' +
                    '<a href="#" data-freeze-link data-file-id="' + esc(f.id) + '" data-frozen="0">Freeze</a> | ' +
                    '<a href="#" data-delete-link data-file-id="' + esc(f.id) + '">Delete</a>' +
                "</td></tr>";
        }
        return html;
    }

    function updateStatus() {
        var statusEl = document.getElementById("files-status");
        if (!statusEl) return;
        var filtered = filterByType(allFiles);
        if (currentTotal > allFiles.length) {
            statusEl.textContent = "Showing " + Math.min(displayedCount, filtered.length) + " of " + filtered.length + " matched (total: " + currentTotal + ")";
        } else {
            statusEl.textContent = filtered.length + " file" + (filtered.length !== 1 ? "s" : "");
        }
    }

    function renderTable() {
        var listEl = document.getElementById("files-list");
        if (!listEl) return;
        var filtered = filterByType(allFiles);
        var toShow = filtered.slice(0, displayedCount);
        if (!toShow.length && !allFiles.length) {
            listEl.innerHTML = "<p>No files uploaded yet.</p>";
            document.getElementById("files-status").textContent = "";
            return;
        }
        var html = '<div class="table-wrap"><table>' +
            '<thead><tr>' +
            '<th>ID</th><th>Filename</th><th>Size</th><th>Type</th>' +
            '<th>Uploaded</th><th>Expires</th><th>TTL</th>' +
            '<th>Views</th><th>Tier</th><th>Protection</th>' +
            '<th>IP Hash</th><th>Actions</th>' +
            "</tr></thead><tbody>" +
            renderRows(toShow) +
            "</tbody></table></div>";
        if (displayedCount < filtered.length) {
            html += '<div style="text-align:center;margin:0.75rem 0">' +
                '<button class="btn btn-primary" id="load-more-btn">Load More (' +
                displayedCount + " / " + filtered.length + ")</button></div>";
        }
        listEl.innerHTML = html;
        var loadMoreBtn = document.getElementById("load-more-btn");
        if (loadMoreBtn) {
            loadMoreBtn.addEventListener("click", function () {
                displayedCount = Math.min(displayedCount + PAGE, filterByType(allFiles).length);
                renderTable();
                updateStatus();
            });
        }
        updateStatus();
    }

    async function fetchData(params) {
        var listEl = document.getElementById("files-list");
        var qs = "q=" + encodeURIComponent(params.q) +
            "&sort=" + encodeURIComponent(params.sort) +
            "&dir=" + encodeURIComponent(params.dir) +
            "&offset=" + params.offset +
            "&limit=" + params.limit;
        try {
            var res = await fetch("/api/admin/files?" + qs);
            if (!res.ok) {
                if (res.status === 401) {
                    listEl.innerHTML = '<p>Not authenticated. <a href="/admin/login">Login</a></p>';
                } else {
                    listEl.innerHTML = "<p>Error: " + res.status + "</p>";
                }
                return null;
            }
            return await res.json();
        } catch (e) {
            listEl.innerHTML = "<p>Failed to load files.</p>";
            return null;
        }
    }

    async function doSearch() {
        var params = buildParams();
        allFiles = [];
        displayedCount = 0;
        currentTotal = 0;
        renderTable();
        document.getElementById("files-list").innerHTML = "<p>Loading...</p>";

        var result = await fetchData(params);
        if (!result) return;
        allFiles = result.items;
        currentTotal = result.total;
        displayedCount = Math.min(PAGE, allFiles.length);
        renderTable();
    }

    async function init() {
        var listEl = document.getElementById("files-list");
        if (!listEl) return;

        listEl.addEventListener("click", function (e) {
            var link = e.target.closest("[data-delete-link]");
            if (link) {
                e.preventDefault();
                var id = link.getAttribute("data-file-id");
                if (id) deleteFile(id);
            }
            var tierLink = e.target.closest("[data-tier-toggle]");
            if (tierLink) {
                e.preventDefault();
                var tierId = tierLink.getAttribute("data-file-id");
                var action = tierLink.getAttribute("data-tier-action") === "cool" ? "cool" : "warm";
                if (tierId && confirm((action === "cool" ? "Pull file " : "Force-promote file ") + tierId + (action === "cool" ? " back from hot tier?" : " to hot tier now?"))) tierAction(tierId, action);
            }
            var freezeLink = e.target.closest("[data-freeze-link]");
            if (freezeLink) {
                e.preventDefault();
                var freezeId = freezeLink.getAttribute("data-file-id");
                var frozen = freezeLink.getAttribute("data-frozen") === "1";
                if (freezeId && confirm((frozen ? "Unfreeze file " : "Freeze file ") + freezeId + "?")) {
                    toggleFreeze(freezeId, frozen, freezeLink);
                }
            }
            var banLink = e.target.closest("[data-ban-trigger]");
            if (banLink) {
                e.preventDefault();
                var hash = banLink.getAttribute("data-ban-hash");
                if (hash && confirm("Ban IP with hash " + hash + "?")) {
                    fetch("/api/admin/bans", {
                        method: "POST",
                        headers: { "Content-Type": "application/json" },
                        body: JSON.stringify({ hash: hash, reason: "Banned from files admin" }),
                    }).then(function (r) {
                        if (r.ok) alert("IP banned.");
                        else alert("Ban failed: " + r.status);
                    });
                }
            }
        });

        document.getElementById("search-btn").addEventListener("click", doSearch);
        document.getElementById("search-input").addEventListener("keydown", function (e) {
            if (e.key === "Enter") doSearch();
        });
        document.getElementById("sort-select").addEventListener("change", doSearch);
        document.getElementById("dir-select").addEventListener("change", doSearch);
        document.getElementById("filter-type").addEventListener("change", function () {
            displayedCount = Math.min(PAGE, filterByType(allFiles).length);
            renderTable();
        });

        await doSearch();
    }

    async function tierAction(id, action) {
        try {
            var res = await fetch("/api/admin/file/" + encodeURIComponent(id) + "/" + action, { method: "POST" });
            if (!res.ok) throw new Error("HTTP " + res.status);
            var body = await res.json().catch(function () { return {}; });
            var entry = allFiles.find(function (f) { return f.id === id; });
            if (entry) {
                entry.tier = body.tier || (action === "warm" ? "hot" : "standard");
                entry.hot = entry.tier === "hot";
                entry.tier_state = entry.tier;
            }
            renderTable();
        } catch (err) {
            alert("Tier change failed: " + (err && err.message ? err.message : err));
        }
    }

    async function toggleFreeze(id, frozen, linkEl) {
        try {
            var res = await fetch("/api/admin/file/" + encodeURIComponent(id) + (frozen ? "/unfreeze" : "/freeze"), { method: "POST" });
            if (!res.ok) throw new Error("HTTP " + res.status);
            if (linkEl) {
                linkEl.setAttribute("data-frozen", frozen ? "0" : "1");
                linkEl.textContent = frozen ? "Freeze" : "Unfreeze";
            }
        } catch (err) {
            alert("Freeze toggle failed: " + (err && err.message ? err.message : err));
        }
    }

    async function deleteFile(id) {
        if (!confirm("Delete file " + id + "?")) return;
        try {
            var res = await fetch("/api/admin/file/" + encodeURIComponent(id), { method: "DELETE" });
            if (res.ok) {
                allFiles = allFiles.filter(function (f) { return f.id !== id; });
                displayedCount = Math.min(displayedCount, filterByType(allFiles).length);
                renderTable();
                updateStatus();
            } else {
                var body = await res.text();
                alert("Delete failed: " + (body || res.status));
            }
        } catch (e) {
            alert("Delete failed: " + e.message);
        }
    }

    init();
