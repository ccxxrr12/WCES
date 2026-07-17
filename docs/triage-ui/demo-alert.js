// WCES Demo — Breathing distress + fall alert scenario
// SPEED: playback speed factor (1=real-time, 0.5=half speed, 2=double speed)
(function(){
var DEMO_t=0,scenIdx=0,DEMO_alerts=[],DEMO_done=false,TICK_SPEED=0.35;
var SCENARIO=[
    // Phase 1: Normal — stable vital signs (0-12s demo time, ~34s real)
    {t:0,br:18,hr:72,triage:'Delayed',color:'yellow',deter:false,alert:null},
    // Phase 2: Early deterioration — breathing rises (12-24s demo, ~34s real)
    {t:12,br:26,hr:78,triage:'Delayed',color:'yellow',deter:false,alert:{time:'02:30:15',survivor_id:'S1',message:'呼吸率 18→26 BPM，持续监测中',priority:2}},
    // Phase 3: Crossing threshold — tachypnea triggers re-triage (24-36s demo)
    {t:24,br:35,hr:95,triage:'Immediate',color:'red',deter:true,alert:{time:'02:30:27',survivor_id:'S1',message:'呼吸率 26→35 BPM, START Delayed→Immediate',priority:2}},
    // Phase 4: Peak severity — tachycardia sets in (36-48s demo)
    {t:36,br:37,hr:112,triage:'Immediate',color:'red',deter:true,alert:null},
    // Phase 5: Critical — heart rate exceeds 120 (48-60s demo)
    {t:48,br:38,hr:132,triage:'Immediate',color:'red',deter:true,alert:{time:'02:30:45',survivor_id:'S1',message:'心率 >120 BPM. Medical Agent 深度分析触发',priority:1}},
    // Phase 6: Fall event (60-72s demo)
    {t:60,br:40,hr:135,triage:'Immediate',color:'red',deter:true,alert:{time:'02:30:57',survivor_id:'S1',message:'跌倒检测触发 (accel 32.4 > 15.0)',priority:1}},
    // Phase 7: Stabilization begins (72-90s demo)
    {t:72,br:28,hr:98,triage:'Delayed',color:'yellow',deter:false,alert:{time:'02:31:15',survivor_id:'S1',message:'体征逐步恢复，START Immediate→Delayed',priority:2}},
    // Phase 8: Recovery — back to baseline (90-105s demo)
    {t:90,br:18,hr:72,triage:'Delayed',color:'yellow',deter:false,alert:null}
];
function tick(dt){
    dt=Math.min(dt||0.05,0.15)*TICK_SPEED;DEMO_t+=dt;
    if(scenIdx<SCENARIO.length&&DEMO_t>=SCENARIO[scenIdx].t){
        var s=SCENARIO[scenIdx];if(s.alert)DEMO_alerts.push(s.alert);scenIdx++;
    }
    if(scenIdx>=SCENARIO.length){scenIdx=SCENARIO.length-1} // hold last state, no loop
    var s=SCENARIO[Math.min(scenIdx,SCENARIO.length-1)];
    if(scenIdx>0)for(var i=scenIdx-1;i>=0;i--){if(SCENARIO[i].t<=DEMO_t){s=SCENARIO[i];break}}
    var br=s.br+Math.random()*2-1,hr=s.hr+Math.random()*4-2,det=s.deter;
    handleUpdate({type:'sensing_update',source:'esp32',tick:Math.round(DEMO_t*10),
        nodes:[
            {node_id:1,rssi_dbm:-52,position:[0,2.0,1.0],amplitude:Array(242).fill(12),subcarrier_count:242,breathing_rate_bpm:br,heart_rate_bpm:hr,motion_level:'present_still',presence:true,active:true,channel:149,band:'5GHz'},
            {node_id:2,rssi_dbm:-55,position:[-2.2,-1.5,1.0],amplitude:Array(242).fill(6),subcarrier_count:242,breathing_rate_bpm:null,heart_rate_bpm:null,motion_level:'absent',presence:true,active:true,channel:149,band:'5GHz'},
            {node_id:3,rssi_dbm:-53,position:[2.2,-1.5,1.0],amplitude:Array(242).fill(6),subcarrier_count:242,breathing_rate_bpm:null,heart_rate_bpm:null,motion_level:'absent',presence:true,active:true,channel:149,band:'5GHz'}
        ],
        features:{mean_rssi:-52,variance:det?1800:1200,motion_band_power:det?40:15,breathing_band_power:det?25:8,dominant_freq_hz:det?0.45:0.25,change_points:det?15:5,spectral_power:det?800:400},
        classification:{motion_level:det?'present_still':'absent',presence:true,confidence:0.75},
        signal_field:{grid_size:[20,1,20],values:Array(400).fill(0.05)},
        vital_signs:{breathing_rate_bpm:br,heart_rate_bpm:hr,breathing_confidence:0.55,heartbeat_confidence:0.35,signal_quality:0.45},
        triage_update:{survivors:[{id:'S1',triage:s.triage,triage_color:s.color,triage_priority:s.color==='red'?1:2,breathing_rate:br,heart_rate:hr,motion_score:det?0.4:0.15,position:[0,2.0,1.0],position_confidence:0.65,is_deteriorating:det,tracked_seconds:200+DEMO_t,node_id:1,estimated_age:'Adult',status:'active',reidentified:false}],assessment:{total:1,immediate:s.color==='red'?1:0,delayed:s.color==='yellow'?1:0,minor:s.color==='green'?1:0,deceased:0,unknown:0,severity:s.color==='red'?'Critical':'Moderate',rescuer_estimate:s.color==='red'?4:2},alerts:s.color==='red'?DEMO_alerts.slice(-2):[]},
        wasm_alerts:det?[{module:'fall_detect',severity:'critical',event_name:'fall_detected',value:32.4}]:[],pose_keypoints:null,model_status:null,persons:null,estimated_persons:1,tracked_survivors:null,alerts:null
    });
}
function loop(t){if(DEMO_done)return;tick(t?t*0.001:0.05);requestAnimationFrame(loop)}
document.getElementById('statusDot').className='status-dot online';
document.getElementById('statusText').textContent='已连接';
requestAnimationFrame(loop);
window.addEventListener('beforeunload',function(){DEMO_done=true;});
})();
