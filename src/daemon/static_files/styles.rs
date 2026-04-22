pub(super) const NOT_FOUND_CSS: &str = include_str!("../templates/file_not_found.css");

pub(super) const FILEBROWSER_CSS: &str = "\
.page-title{\
    font-size:1.1em;color:var(--text-light);margin-bottom:16px;\
    font-weight:400;display:flex;align-items:center;gap:8px;\
}\
.page-title code{\
    font-size:1em;font-weight:600;color:var(--text);\
    background:transparent;padding:0;\
}\
.breadcrumb{\
    display:flex;flex-wrap:wrap;align-items:center;gap:4px;\
    padding:12px 18px;margin-bottom:24px;\
    background:var(--card-bg);border-radius:10px;\
    border:1px solid var(--border);font-size:.9em;\
    box-shadow:0 2px 8px rgba(0,0,0,.03);\
}\
.breadcrumb a{\
    color:var(--fox-orange);padding:4px 8px;border-radius:6px;\
    transition:all .2s ease;\
}\
.breadcrumb a:hover{\
    background:rgba(232,133,58,.12);text-decoration:none;\
    transform:translateY(-1px);\
}\
.bc-home{vertical-align:middle;color:var(--fox-orange)}\
.breadcrumb .sep{color:var(--border-hover);margin:0 4px;font-size:.85em}\
.file-card{\
    background:var(--card-bg);border-radius:12px;\
    border:1px solid var(--border);overflow:hidden;\
    box-shadow:0 4px 16px rgba(0,0,0,.04);\
    animation:fadeIn .5s ease-out;\
}\
.file-card table{width:100%;border-collapse:collapse}\
.file-card th{\
    text-align:left;padding:14px 18px;\
    background:linear-gradient(180deg,#FEFAF6 0%,#FDF6EE 100%);\
    color:var(--text-light);\
    font-size:.75em;font-weight:600;\
    text-transform:uppercase;letter-spacing:.08em;\
    cursor:pointer;user-select:none;\
    border-bottom:2px solid var(--border);\
    transition:all .2s ease;\
}\
.file-card th:hover{color:var(--fox-orange);background:#FFF5ED}\
.file-card td{\
    padding:12px 18px;border-bottom:1px solid #FAF4EE;\
    vertical-align:middle;transition:background .2s ease;\
}\
.file-card tr:last-child td{border-bottom:none}\
.file-card tbody tr:hover td{background:#FFF8F2;cursor:pointer}\
.file-card a{color:var(--text);font-weight:500;transition:all .2s ease}\
.file-card a:hover{color:var(--fox-orange);text-decoration:none}\
.parent-row td{padding:10px 18px;background:rgba(232,133,58,.03)}\
.parent-row:hover td{background:rgba(232,133,58,.08)!important}\
.ei{vertical-align:middle;margin-right:10px;transition:transform .2s}\
.file-card tr:hover .ei{transform:scale(1.1)}\
.size,.modified{\
    color:var(--text-light);\
    font-family:'SF Mono',Monaco,'Cascadia Code',Menlo,Consolas,monospace;\
    font-size:.82em;white-space:nowrap;\
}\
.si{font-size:.75em;margin-left:6px;opacity:.3;transition:all .2s}\
.si.active{opacity:1;color:var(--fox-orange);font-weight:700}\
.empty-dir{padding:48px 18px;text-align:center;color:var(--text-light);font-style:italic}\
.col-size{width:110px}\
.col-mod{width:200px}\
@keyframes fadeIn{from{opacity:0}to{opacity:1}}\
@media(max-width:768px){.col-mod{display:none}.col-size{width:80px}}\
";

pub(super) const FILEBROWSER_JS: &str = "\
document.querySelectorAll('.modified').forEach(function(el){\
    var ts=parseInt(el.dataset.ts);\
    if(ts>0){\
        var d=new Date(ts*1000);\
        el.textContent=d.toLocaleDateString(undefined,\
            {year:'numeric',month:'short',day:'numeric'})\
            +' '+d.toLocaleTimeString([],{hour:'2-digit',minute:'2-digit'});\
    }\
});\
var col=0,asc=true;\
function sort(c){\
    if(col===c)asc=!asc;else{col=c;asc=true;}\
    for(var i=0;i<3;i++){\
        var el=document.getElementById('s'+i);\
        el.className='si'+(i===col?' active':'');\
        el.textContent=i===col?(asc?'\\u25B2':'\\u25BC'):'';\
    }\
    var tbody=document.querySelector('#listing tbody');\
    var nonEntryRows=Array.from(tbody.querySelectorAll('tr:not([data-name])'));\
    var rows=Array.from(tbody.querySelectorAll('tr[data-name]'));\
    rows.sort(function(a,b){\
        var ad=parseInt(a.dataset.dir),bd=parseInt(b.dataset.dir);\
        if(ad!==bd)return bd-ad;\
        var av,bv;\
        if(c===0){\
            av=a.dataset.name.toLowerCase();\
            bv=b.dataset.name.toLowerCase();\
            return asc?av.localeCompare(bv):bv.localeCompare(av);\
        }else if(c===1){\
            av=parseInt(a.dataset.size);bv=parseInt(b.dataset.size);\
        }else{\
            av=parseInt(a.dataset.ts);bv=parseInt(b.dataset.ts);\
        }\
        return asc?av-bv:bv-av;\
    });\
    while(tbody.firstChild)tbody.removeChild(tbody.firstChild);\
    nonEntryRows.forEach(function(r){tbody.appendChild(r);});\
    rows.forEach(function(r){tbody.appendChild(r);});\
}\
";
