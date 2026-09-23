const params = new URLSearchParams(location.search);
const msg = params.get("msg");
if (msg) document.getElementById("msg").textContent = msg;
