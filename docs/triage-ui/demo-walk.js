// WCES Demo Walk — N1→N2→N3→center (10s walk, 7s pause, one cycle)
(function(){
var T=0,wp=0,seg=0,phase=0,done=0,lastT=0;
var pts=[{x:0,y:2.0},{x:-2.2,y:-1.5},{x:2.2,y:-1.5},{x:0,y:0}];
var sx=0,sy=2.0;

function tick(dt_sec){
    if(done)return;
    T+=dt_sec;
    if(phase===0){ // pause
        seg+=dt_sec;
        if(seg>=7){seg=0;phase=1;wp=(wp+1)%4;if(wp===3)done=1;}
    }else{ // walk
        seg+=dt_sec;
        var t=Math.min(seg/10,1);
        var f=pts[(wp-1+4)%4],g=pts[wp];
        sx=f.x+(g.x-f.x)*t;sy=f.y+(g.y-f.y)*t;
        if(t>=1){seg=0;phase=0}
    }
    var d1=Math.hypot(sx,sy-2),d2=Math.hypot(sx+2.2,sy+1.5),d3=Math.hypot(sx-2.2,sy+1.5);
    var nn=d1<d2?(d1<d3?1:3):(d2<d3?2:3);
    var tc=nn===1?'yellow':'green',tl=nn===1?'Delayed':'Minor';
    var br=16+3*Math.sin(T*.5)+Math.random()*1.5,hr=70+5*Math.sin(T*.7)+Math.random()*3;
    handleUpdate({type:'sensing_update',source:'esp32',tick:Math.round(T*10),
        nodes:[
            {node_id:1,rssi_dbm:-52,position:[0,2,1],amplitude:Array(56).fill(12),subcarrier_count:56,breathing_rate_bpm:nn===1?br:null,heart_rate_bpm:nn===1?hr:null,motion_level:'present_still',presence:!0,active:!0,channel:149,band:'5GHz'},
            {node_id:2,rssi_dbm:-55,position:[-2.2,-1.5,1],amplitude:Array(56).fill(8),subcarrier_count:56,breathing_rate_bpm:nn===2?br:null,heart_rate_bpm:nn===2?hr:null,motion_level:'absent',presence:!0,active:!0,channel:149,band:'5GHz'},
            {node_id:3,rssi_dbm:-53,position:[2.2,-1.5,1],amplitude:Array(56).fill(8),subcarrier_count:56,breathing_rate_bpm:nn===3?br:null,heart_rate_bpm:nn===3?hr:null,motion_level:'absent',presence:!0,active:!0,channel:149,band:'5GHz'}
        ],
        features:{mean_rssi:-52,variance:1200,motion_band_power:15,breathing_band_power:8,dominant_freq_hz:.25,change_points:5,spectral_power:400},
        classification:{motion_level:'present_still',presence:!0,confidence:.75},
        signal_field:{grid_size:[20,1,20],values:Array(400).fill(.05)},
        vital_signs:{breathing_rate_bpm:br,heart_rate_bpm:hr,breathing_confidence:.55,heartbeat_confidence:.35,signal_quality:.45},
        triage_update:{survivors:[{id:'S1',triage:tl,triage_color:tc,triage_priority:tc==='yellow'?2:3,breathing_rate:br,heart_rate:hr,motion_score:.15,position:[sx,sy,1],position_confidence:.65,is_deteriorating:!1,tracked_seconds:T,node_id:nn,estimated_age:'Adult',status:'active',reidentified:!1}],assessment:{total:1,immediate:0,delayed:tc==='yellow'?1:0,minor:tc==='green'?1:0,deceased:0,unknown:0,severity:tc==='yellow'?'Moderate':'Minimal',rescuer_estimate:2},alerts:[]},
        wasm_alerts:[],pose_keypoints:null,model_status:null,persons:null,estimated_persons:1,tracked_survivors:null,alerts:null
    });
    drawMap();
}
function loop(ts){
    if(done)return;
    var dt=(lastT?(ts-lastT)*0.001:0.016);
    dt=Math.min(dt,0.2);
    lastT=ts;
    tick(dt);
    requestAnimationFrame(loop);
}
document.getElementById('statusDot').className='status-dot online';
document.getElementById('statusText').textContent='已连接';
requestAnimationFrame(loop);
})();
