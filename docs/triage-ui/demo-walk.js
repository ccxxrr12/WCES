// WCES Demo — N1→N2→N3→center (3s walk, 2s pause, one cycle)
(function(){
var DEMO_t=0,wpIdx=0,segT=0,phase='pause',done=false;
var WP=[{x:0,y:2.0},{x:-2.2,y:-1.5},{x:2.2,y:-1.5},{x:0,y:0}];
var survX=0,survY=2.0,prevWp=3;

function tick(dt){
    dt=Math.min(dt||0.05,0.15);
    if(done)return;
    DEMO_t+=dt;
    if(phase==='pause'){
        segT+=dt;
        if(segT>=2){segT=0;phase='walk';if(wpIdx===3)done=true;else wpIdx=(wpIdx+1)%WP.length}
    }else{
        segT+=dt;
        var t=Math.min(segT/3,1);
        var from=WP[(wpIdx-1+WP.length)%WP.length],to=WP[wpIdx];
        survX=from.x+(to.x-from.x)*t;survY=from.y+(to.y-from.y)*t;
        if(t>=1){segT=0;phase='pause'}
    }
    var d1=Math.hypot(survX,survY-2.0),d2=Math.hypot(survX+2.2,survY+1.5),d3=Math.hypot(survX-2.2,survY+1.5);
    var nn=d1<d2?(d1<d3?1:3):(d2<d3?2:3);
    var tc=nn===1?'yellow':'green',tl=nn===1?'Delayed':'Minor';
    var br=16+3*Math.sin(DEMO_t*0.5)+Math.random()*1.5,hr=70+5*Math.sin(DEMO_t*0.7)+Math.random()*3;
    handleUpdate({type:'sensing_update',source:'esp32',tick:Math.round(DEMO_t*10),
        nodes:[
            {node_id:1,rssi_dbm:-52,position:[0,2.0,1.0],amplitude:Array(56).fill(12),subcarrier_count:56,breathing_rate_bpm:br,heart_rate_bpm:hr,motion_level:'present_still',presence:true,active:true,channel:149,band:'5GHz'},
            {node_id:2,rssi_dbm:-55,position:[-2.2,-1.5,1.0],amplitude:Array(56).fill(8),subcarrier_count:56,breathing_rate_bpm:null,heart_rate_bpm:null,motion_level:'absent',presence:true,active:true,channel:149,band:'5GHz'},
            {node_id:3,rssi_dbm:-53,position:[2.2,-1.5,1.0],amplitude:Array(56).fill(8),subcarrier_count:56,breathing_rate_bpm:null,heart_rate_bpm:null,motion_level:'absent',presence:true,active:true,channel:149,band:'5GHz'}
        ],
        features:{mean_rssi:-52,variance:1200,motion_band_power:15,breathing_band_power:8,dominant_freq_hz:0.25,change_points:5,spectral_power:400},
        classification:{motion_level:'present_still',presence:true,confidence:0.75},
        signal_field:{grid_size:[20,1,20],values:Array(400).fill(0.05)},
        vital_signs:{breathing_rate_bpm:br,heart_rate_bpm:hr,breathing_confidence:0.55,heartbeat_confidence:0.35,signal_quality:0.45},
        triage_update:{survivors:[{id:'S1',triage:tl,triage_color:tc,triage_priority:tc==='yellow'?2:3,breathing_rate:br,heart_rate:hr,motion_score:0.15,position:[survX,survY,1.0],position_confidence:0.65,is_deteriorating:false,tracked_seconds:DEMO_t,node_id:nn,estimated_age:'Adult',status:'active',reidentified:false}],assessment:{total:1,immediate:0,delayed:tc==='yellow'?1:0,minor:tc==='green'?1:0,deceased:0,unknown:0,severity:tc==='yellow'?'Moderate':'Minimal',rescuer_estimate:2},alerts:[]},
        wasm_alerts:[],pose_keypoints:null,model_status:null,persons:null,estimated_persons:1,tracked_survivors:null,alerts:null
    });
    drawMap();
}
document.getElementById('statusDot').className='status-dot online';
document.getElementById('statusText').textContent='已连接';
function loop(t){tick(t?t*0.001:0.05);requestAnimationFrame(loop)}
requestAnimationFrame(loop);
})();
