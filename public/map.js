// Leaflet glue for the settings-page location picker. Defined globally (loaded
// from the document head) so the WASM client can call it. Leaflet itself is
// loaded only on the settings page; these helpers retry until it's available.
(function () {
  function whenReady(elId, cb, tries) {
    tries = tries || 0;
    var el = document.getElementById(elId);
    if (window.L && el) return cb(el);
    if (tries > 100) return; // ~10s give-up
    setTimeout(function () { whenReady(elId, cb, tries + 1); }, 100);
  }

  // Create (once) a map + draggable marker on `elId`, centered on lat/lon.
  // `onPick(lat, lon)` is called when the user clicks the map or drags the marker.
  window.birdnetInitMap = function (elId, lat, lon, onPick) {
    whenReady(elId, function (el) {
      var has = lat || lon;
      var center = [Number(lat) || 0, Number(lon) || 0];
      if (el._birdnetMap) {
        el._birdnetMap.invalidateSize();
        el._birdnetMarker.setLatLng(center);
        el._birdnetMap.setView(center, has ? 9 : 2);
        return;
      }
      var map = L.map(el).setView(has ? center : [20, 0], has ? 9 : 2);
      L.tileLayer("https://{s}.tile.openstreetmap.org/{z}/{x}/{y}.png", {
        maxZoom: 19,
        attribution: "&copy; OpenStreetMap contributors",
      }).addTo(map);
      var marker = L.marker(center, { draggable: true }).addTo(map);
      function pick(ll) {
        marker.setLatLng(ll);
        if (onPick) onPick(ll.lat, ll.lng);
      }
      map.on("click", function (e) { pick(e.latlng); });
      marker.on("dragend", function () {
        var ll = marker.getLatLng();
        if (onPick) onPick(ll.lat, ll.lng);
      });
      el._birdnetMap = map;
      el._birdnetMarker = marker;
      // Tiles can render blank if the container sized after init.
      setTimeout(function () { map.invalidateSize(); }, 200);
    });
  };

  // Move the marker/view (used when the lat/lon inputs are edited by hand).
  window.birdnetSetMarker = function (elId, lat, lon) {
    var el = document.getElementById(elId);
    if (el && el._birdnetMap) {
      var ll = [Number(lat) || 0, Number(lon) || 0];
      el._birdnetMarker.setLatLng(ll);
      el._birdnetMap.panTo(ll);
    }
  };
})();
