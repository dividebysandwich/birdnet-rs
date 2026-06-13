// Chart.js glue for the calendar + statistics pages. Defined globally (loaded
// from the document head) so the WASM client can call it. Chart.js itself is
// loaded only on those pages; this helper retries until it's available.
(function () {
  function whenReady(elId, cb, tries) {
    tries = tries || 0;
    var el = document.getElementById(elId);
    if (window.Chart && el) return cb(el);
    if (tries > 100) return; // ~10s give-up
    setTimeout(function () { whenReady(elId, cb, tries + 1); }, 100);
  }

  var ACCENT = "#3a86ff";
  var MUTED = "#8b93a7";
  var GRID = "rgba(255,255,255,0.06)";

  // Render (or re-render) a chart on canvas `elId`. `type` is "bar" | "line".
  // `labelsJson`/`valuesJson` are JSON-encoded arrays; `label` is the dataset
  // legend/tooltip title. Horizontal bars for "bar" (species breakdowns).
  window.birdnetChart = function (elId, type, labelsJson, valuesJson, label) {
    whenReady(elId, function (el) {
      var labels, values;
      try {
        labels = JSON.parse(labelsJson);
        values = JSON.parse(valuesJson);
      } catch (e) { return; }

      if (el._chart) el._chart.destroy();

      var horizontal = type === "bar";
      el._chart = new window.Chart(el, {
        type: type,
        data: {
          labels: labels,
          datasets: [{
            label: label || "Detections",
            data: values,
            backgroundColor: horizontal ? ACCENT : "rgba(58,134,255,0.15)",
            borderColor: ACCENT,
            borderWidth: horizontal ? 0 : 2,
            borderRadius: horizontal ? 3 : 0,
            pointRadius: horizontal ? 0 : 3,
            pointBackgroundColor: ACCENT,
            fill: !horizontal,
            tension: 0.25,
          }],
        },
        options: {
          indexAxis: horizontal ? "y" : "x",
          responsive: true,
          maintainAspectRatio: false,
          plugins: { legend: { display: false } },
          scales: {
            x: {
              ticks: { color: MUTED, precision: 0 },
              grid: { color: GRID },
              beginAtZero: true,
            },
            y: {
              ticks: { color: MUTED, precision: 0 },
              grid: { color: GRID },
              beginAtZero: true,
            },
          },
        },
      });
    });
  };
})();
