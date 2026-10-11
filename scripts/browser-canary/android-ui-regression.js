const fs=require('fs');
const {chromium}=require('patchright');
(async()=>{
 const token=fs.readFileSync('/tmp/kaleido_token.txt','utf8').trim();
 const ua='Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/128.0.0.0 Mobile Safari/537.36';
 const b=await chromium.launch({headless:true});
 const c=await b.newContext({userAgent:ua, viewport:{width:412,height:915}, screen:{width:412,height:915}, deviceScaleFactor:2, isMobile:true, hasTouch:true, locale:'zh-CN', timezoneId:'Asia/Shanghai', extraHTTPHeaders:{Authorization:`Bearer ${token}`}, storageState:{cookies:[],origins:[{origin:'http://127.0.0.1:18766',localStorage:[{name:'kaleido_token',value:token},{name:'token',value:token},{name:'auth_token',value:token},{name:'museai_token',value:token}]}]}});
 const p=await c.newPage(); const errors=[]; const bad=[]; const responses=[];
 p.on('console',m=>{if(m.type()==='error') errors.push({text:m.text(),url:m.location().url})});
 p.on('pageerror',e=>errors.push({text:'PAGEERROR '+String(e)}));
 p.on('response',r=>{if(r.status()>=400){bad.push({status:r.status(),method:r.request().method(),url:r.url()})}});
 await p.goto('http://127.0.0.1:18766/web/',{waitUntil:'networkidle',timeout:60000});
 const outDir=process.env.MUSEAI_CANARY_OUT || '/tmp/museai-browser-canary/artifacts/android-regression'; fs.mkdirSync(outDir,{recursive:true});
 async function shot(name){await p.screenshot({path:`${outDir}/${name}.png`,fullPage:true}); return {name,url:p.url(),title:await p.title(),body:(await p.locator('body').innerText()).slice(0,2500)};}
 const shots=[]; shots.push(await shot('01-home'));
 async function click(name){const loc=p.getByRole('button',{name:new RegExp(name)}).first(); if(await loc.count()){await loc.click(); await p.waitForTimeout(700); return true} return false}
 await click('故事馆'); shots.push(await shot('02-story'));
 await click('对话'); shots.push(await shot('03-dialogue'));
 const msg=p.locator('input[placeholder*="问助手"],textarea[placeholder*="消息"],textarea[placeholder*="输入"],input[placeholder*="输入"]').last();
 let sendAttempt=false; if(await msg.count() && await msg.isVisible().catch(()=>false)){await msg.fill('UI 测试：请仅返回“收到”'); sendAttempt=await click('发送'); if(sendAttempt) await p.waitForTimeout(5000);}
 shots.push(await shot('04-dialogue-after-send'));
 await click('作者区'); shots.push(await shot('05-author'));
 await click('更多'); shots.push(await shot('06-more'));
 const result={ua,viewport:{width:412,height:915},url:p.url(),title:await p.title(),webdriver:await p.evaluate(()=>navigator.webdriver),shots,sendAttempt,badRequests:bad,consoleErrors:errors,storageKeys:await p.evaluate(()=>Object.keys(localStorage)),artifactDir:outDir};
 fs.writeFileSync(`${outDir}/result.json`,JSON.stringify(result,null,2)); console.log(JSON.stringify(result,null,2)); await b.close();
})().catch(e=>{console.error(e);process.exit(1)})
