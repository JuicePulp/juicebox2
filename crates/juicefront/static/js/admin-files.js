
    var PAGE = 200;
    var allFiles = [];
    var displayedCount = 0;
    var currentTotal = 0;
    var searchTimer = null;
    var filesObserver = null;
    var isLoadingMore = false;
    var currentQuery = { q: "", sort: "uploaded_at", dir: "desc" };

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

    function buildParams(offset) {
        var q = document.getElementById("search-input").value.trim();
        var sort = document.getElementById("sort-select").value;
        var dir = document.getElementById("dir-select").value;
        return { q: q, sort: sort, dir: dir, offset: offset || 0, limit: PAGE };
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
                "<td>" + (f.encrypted ? "Yes" : (f.protected ? "Locked" : "No")) + "</td>" +
                "<td>" +
                    (f.uploader_ip_hash
                        ? '<a href="#" data-ban-trigger data-ban-hash="' + esc(f.uploader_ip_hash) + '">' + esc(f.uploader_ip_hash) + "</a>"
                        : "???") +
                    (f.uploader_geo ? '<br><span class="tier-standard">' + esc(f.uploader_geo) + "</span>" : "") +
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
        if (isLoadingMore) {
            statusEl.textContent = "Loading more... (" + allFiles.length + " / " + currentTotal + ")";
            return;
        }
        var filtered = filterByType(allFiles);
        if (currentTotal > allFiles.length) {
            statusEl.textContent = "Showing " + Math.min(displayedCount, filtered.length) + " of " + filtered.length + " loaded (total: " + currentTotal + ") - scroll for more";
        } else {
            statusEl.textContent = filtered.length + " file" + (filtered.length !== 1 ? "s" : "");
        }
    }

    function loadMore() {
        var filteredLen = filterByType(allFiles).length;
        if (displayedCount < filteredLen) {
            displayedCount = Math.min(displayedCount + PAGE, filteredLen);
            renderTable();
            return true;
        }
        if (allFiles.length < currentTotal) {
            fetchMore();
            return true;
        }
        return false;
    }

    function hasMore() {
        return displayedCount < filterByType(allFiles).length || allFiles.length < currentTotal;
    }

    async function fetchPageSilent(params) {
        var qs = "q=" + encodeURIComponent(params.q) +
            "&sort=" + encodeURIComponent(params.sort) +
            "&dir=" + encodeURIComponent(params.dir) +
            "&offset=" + params.offset +
            "&limit=" + params.limit;
        try {
            var res = await fetch("/api/admin/files?" + qs);
            if (!res.ok) return null;
            return await res.json();
        } catch (e) {
            return null;
        }
    }

    async function fetchMore() {
        if (isLoadingMore) return;
        if (allFiles.length >= currentTotal) return;
        isLoadingMore = true;
        updateStatus();
        var params = {
            q: currentQuery.q,
            sort: currentQuery.sort,
            dir: currentQuery.dir,
            offset: allFiles.length,
            limit: PAGE
        };
        var result = await fetchPageSilent(params);
        isLoadingMore = false;
        if (!result || !result.items) {
            renderTable();
            return;
        }
        var seen = {};
        var i;
        for (i = 0; i < allFiles.length; i++) seen[allFiles[i].id] = true;
        for (i = 0; i < result.items.length; i++) {
            if (!seen[result.items[i].id]) {
                allFiles.push(result.items[i]);
                seen[result.items[i].id] = true;
            }
        }
        currentTotal = result.total;
        var filteredLen = filterByType(allFiles).length;
        displayedCount = Math.min(displayedCount + PAGE, filteredLen);
        if (displayedCount >= filteredLen && allFiles.length < currentTotal) {
            displayedCount = filteredLen;
        }
        renderTable();
    }

    function renderTable() {
        var listEl = document.getElementById("files-list");
        if (!listEl) return;
        if (filesObserver) {
            filesObserver.disconnect();
            filesObserver = null;
        }
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
        if (hasMore()) {
            if ("IntersectionObserver" in window) {
                html += '<div id="files-sentinel" style="height:1px" aria-hidden="true"></div>';
                if (isLoadingMore) {
                    html += '<div style="text-align:center;margin:0.75rem 0;color:#888">Loading more...</div>';
                }
            } else {
                html += '<div style="text-align:center;margin:0.75rem 0">' +
                    '<button class="btn btn-primary" id="load-more-btn">Load More (' +
                    displayedCount + " / " + filtered.length +
                    (allFiles.length < currentTotal ? "; total " + currentTotal : "") + ")</button></div>";
            }
        }
        listEl.innerHTML = html;
        var loadMoreBtn = document.getElementById("load-more-btn");
        if (loadMoreBtn) {
            loadMoreBtn.addEventListener("click", function () {
                loadMore();
            });
        }
        var sentinel = document.getElementById("files-sentinel");
        if (sentinel && ("IntersectionObserver" in window)) {
            filesObserver = new IntersectionObserver(function (entries) {
                var visible = entries.some(function (en) { return en.isIntersecting; });
                if (visible) {
                    if (filesObserver) {
                        filesObserver.disconnect();
                        filesObserver = null;
                    }
                    loadMore();
                }
            }, { root: null, rootMargin: "400px" });
            filesObserver.observe(sentinel);
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
        var params = buildParams(0);
        currentQuery = { q: params.q, sort: params.sort, dir: params.dir };
        allFiles = [];
        displayedCount = 0;
        currentTotal = 0;
        isLoadingMore = false;
        renderTable();
        document.getElementById("files-list").innerHTML = "<p>Loading...</p>";

        var result = await fetchData(params);
        if (!result) return;
        allFiles = result.items;
        currentTotal = result.total;
        displayedCount = Math.min(PAGE, filterByType(allFiles).length);
        if (displayedCount === 0 && allFiles.length < currentTotal) {
            displayedCount = 0;
        }
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
