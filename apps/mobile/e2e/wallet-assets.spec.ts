import { test, expect, type Page } from '@playwright/test'

const btcAddress = `tb1p${'q'.repeat(58)}`
const xmrAddress = `5${'a'.repeat(94)}`
const freshXmrAddress = `7${'b'.repeat(94)}`
const peer = `02${'a'.repeat(64)}`
const ln = { enabled:true, network:'signet', running:true, keys:'ready', nodeId:`03${'b'.repeat(64)}`, balances:{lightningSats:345340,inboundSats:47000,onchainSats:199616}, channels:[{id:'a'.repeat(64),peer,capacitySats:300000,outboundSats:240000,inboundSats:30000,ready:true,usable:true},{id:'b'.repeat(64),peer:`03${'c'.repeat(64)}`,capacitySats:200000,outboundSats:105340,inboundSats:17000,ready:true,usable:true}], reveals:['Lightning peers see your node ID and payment routing information.'] }
const xmr = {network:'stagenet',enabled:true,keys:'ready',primaryAddress:xmrAddress,balance:{unlockedPiconero:'1234567890123',pendingPiconero:'1'},scannedHeight:2000000,tip:2000000,lastSync:1794560000,syncError:null}
test.afterEach(async({page})=>{await page.unrouteAll({behavior:'ignoreErrors'})})

async function fixture(page: Page) {
  await page.route('**/network', route => route.fulfill({json:{network:'testnet4',confirmations:1,publicChainServer:false,lightning:'available',silentPayments:'code-only',xmrBackend:{backend:'own-node',network:'stagenet',privacy:'own-node',reveals:'Your own Monero node reads blocks.',nodes:['http://127.0.0.1:38081'],proxied:false,tip:2000000,scannedHeight:2000000,error:null},zecBackend:{enabled:true,backend:'own-node',network:'testnet',privacy:'own-node',reveals:'Your own indexer reads the shielded chain.',servers:['http://127.0.0.1:8137'],active:null,software:null,proxied:false,tip:4000000,scannedHeight:4000000,error:null}}}))
  await page.route('**/wallet',route=>route.fulfill({json:{network:'testnet4',balance:{totalSats:411054,spendableSats:391054,confirmedSats:391054,pendingSats:20000},receiveAddress:btcAddress,lastSync:1794560000,syncError:null,activity:[]}}))
  await page.route('**/lock',route=>route.fulfill({json:{state:'locked',autoLockMinutes:15,autoLockRange:[1,60],lockInSecs:0,failedAttempts:0,retryAfterSecs:0,rememberDevice:false,rememberAvailable:true,backupConfirmed:true,minPassphraseChars:8}}))
  await page.route('**/ln/status',route=>route.fulfill({json:ln}))
  await page.route('**/xmr/wallet',route=>route.fulfill({json:xmr}))
  await page.route('**/orders',route=>route.fulfill({json:{orders:[]}}))
  await page.route('**/store/profile',route=>route.fulfill({json:{role:'seller',storeKey:'c'.repeat(64),name:'Trial Store',setUp:true,network:'testnet4'}}))
}
async function openWallet(page: Page) {
  await page.goto('/')
  const opener=page.getByRole('button',{name:'Open navigation',exact:true})
  if(await opener.isVisible()) await opener.click()
  await page.getByRole('button',{name:'Wallet',exact:true}).click()
  await expect(page.getByRole('tab',{name:/^Zcash/})).toBeVisible()
}
async function fits(page: Page) {
  expect(await page.locator('.ob-content').evaluate(el=>el.scrollWidth<=el.clientWidth+1)).toBeTruthy()
  const panel=page.getByRole('tabpanel')
  expect(await panel.evaluate(el=>el.scrollWidth<=el.clientWidth+1)).toBeTruthy()
}

for (const [width,dark] of [[320,false],[812,true],[1440,false],[1920,true]] as const) test(`all enabled wallets fit ${width}px`,async({page,context},info)=>{
  await context.grantPermissions(['clipboard-read','clipboard-write'])
  await page.setViewportSize({width,height:1050})
  await page.emulateMedia({colorScheme:dark?'dark':'light',reducedMotion:'reduce'})
  await fixture(page)
  const mutations:string[]=[]
  page.on('request',request=>{if(request.method()!=='GET' && !request.url().endsWith('/lock/activity'))mutations.push(new URL(request.url()).pathname)})
  let addressCalls=0
  await page.route('**/xmr/wallet/subaddress',route=>{addressCalls++;expect(route.request().headers()['x-ob-pair']).toMatch(/^[a-f0-9]{64}$/i);return route.fulfill({json:{network:'stagenet',index:1,address:freshXmrAddress}})})
  await openWallet(page)
  await expect(page.getByRole('heading',{name:'Your wallet',exact:true})).toHaveCount(1)
  await expect(page.getByRole('tablist',{name:'Wallets'}).getByRole('tab')).toHaveCount(4)
  await fits(page)
  await page.evaluate(async()=>{await document.fonts.ready})
  await page.screenshot({path:info.outputPath(`wallet-bitcoin-${width}.png`)})
  await page.getByRole('tab',{name:/^Lightning/}).click()
  const panel=page.getByRole('tabpanel')
  await expect(panel.getByRole('region',{name:'Lightning balances'})).toContainText('345,340')
  await expect(panel).toContainText('47,000')
  await expect(panel).toContainText('199,616 sats')
  await expect(panel).toContainText('Mutinynet signet · test coins, no value')
  await expect(panel).not.toContainText('$')
  if(width>640) {
    const amounts=await panel.locator('.ob-wallet-metric .ob-wallet-amount strong').all()
    expect(Math.abs((await amounts[0].boundingBox())!.y-(await amounts[1].boundingBox())!.y)).toBeLessThan(1)
  }
  expect(await panel.locator('.ob-wallet-metric .ob-wallet-amount strong').first().evaluate(el=>getComputedStyle(el).fontFamily)).toContain('Instrument Sans')
  await fits(page)
  await page.screenshot({path:info.outputPath(`wallet-lightning-${width}.png`)})
  await panel.getByRole('button',{name:'Copy peer ID',exact:true}).first().click()
  await expect.poll(()=>page.evaluate(()=>navigator.clipboard.readText())).toBe(peer)
  await page.getByRole('tab',{name:/^Monero/}).click()
  await expect(panel.getByRole('region',{name:'Monero balances'})).toContainText('1.234567890123 XMR')
  await expect(panel).toContainText('0.000000000001 XMR')
  await expect(panel).not.toContainText('$')
  await fits(page)
  await page.screenshot({path:info.outputPath(`wallet-monero-${width}.png`)})
  expect(addressCalls).toBe(0)
  await panel.getByRole('button',{name:'New subaddress',exact:true}).click()
  await expect(panel.locator('.ob-copy-value code')).toHaveText(freshXmrAddress)
  expect(addressCalls).toBe(1)
  await panel.getByRole('button',{name:'Receive Monero',exact:true}).click()
  const qr=page.getByRole('dialog',{name:'Receive Monero',exact:true})
  await expect(qr.getByRole('img',{name:'Monero receive QR code'})).toHaveAttribute('data-payment-uri',`monero:${freshXmrAddress}`)
  await qr.getByRole('button',{name:'Copy Monero receive address',exact:true}).click()
  await expect.poll(()=>page.evaluate(()=>navigator.clipboard.readText())).toBe(freshXmrAddress)
  await qr.getByRole('button',{name:'Done',exact:true}).click()
  await page.getByRole('tab',{name:/^Zcash/}).click()
  await expect(panel.getByRole('region',{name:'Zcash receiving'})).toContainText('Receive through your store')
  await expect(panel).toContainText('balance and general receive address aren’t available here yet')
  await expect(panel.getByRole('button',{name:/Receive/})).toHaveCount(0)
  await fits(page)
  await page.screenshot({path:info.outputPath(`wallet-zcash-${width}.png`)})
  await page.getByRole('tab',{name:/^Zcash/}).press('Home')
  await expect(page.getByRole('tab',{name:/^Bitcoin/})).toBeFocused()
  await page.getByRole('tab',{name:/^Bitcoin/}).press('ArrowRight')
  await expect(page.getByRole('tab',{name:/^Lightning/})).toHaveAttribute('aria-selected','true')
  expect(mutations.filter(path=>!path.startsWith('/remote/')&&!path.startsWith('/pair/'))).toEqual(['/xmr/wallet/subaddress'])
})

test('Lightning refresh failures keep the last balances visible',async({page})=>{
  await page.clock.install()
  await fixture(page);await openWallet(page)
  await page.getByRole('tab',{name:/^Lightning/}).click()
  await page.route('**/ln/status',route=>route.fulfill({status:503,json:{error:'Lightning is temporarily unavailable.'}}))
  await page.clock.fastForward(20100)
  const panel=page.getByRole('tabpanel')
  await expect(panel.getByRole('alert')).toContainText('Showing the last available balances.')
  await expect(panel).toContainText('Last known balance')
  await expect(panel).toContainText('345,340')
})

test('Lightning setup shows unavailable balances rather than zero',async({page})=>{
  await fixture(page)
  await page.route('**/ln/status',route=>route.fulfill({json:{...ln,running:false,keys:'unlock-to-create',balances:null,channels:[]}}))
  await openWallet(page)
  await page.getByRole('tab',{name:/^Lightning/}).click()
  const panel=page.getByRole('tabpanel')
  await expect(panel).toContainText('Setup required')
  await expect(panel).toContainText('Balance unavailable')
  await expect(panel.locator('.ob-wallet-amount')).toHaveCount(0)
  await expect(panel).toContainText('Unlock your wallet in Settings')
})

test('Monero preserves a stale balance and rejects receive addresses for a different network',async({page})=>{
  await fixture(page);await openWallet(page)
  await page.getByRole('tab',{name:/^Monero/}).click()
  await page.route('**/xmr/wallet/subaddress',route=>route.fulfill({json:{network:'mainnet',index:1,address:freshXmrAddress}}))
  const panel=page.getByRole('tabpanel')
  await panel.getByRole('button',{name:'New subaddress',exact:true}).click()
  await expect(panel.getByRole('alert')).toContainText('different Monero network')
  await expect(panel.locator('.ob-copy-value code')).toHaveText(xmrAddress)
  await page.route('**/xmr/wallet',route=>route.fulfill({status:503,json:{error:'Monero sync is unavailable.'}}))
  await page.getByRole('button',{name:'Refresh wallets',exact:true}).click()
  await expect(panel.getByRole('heading',{name:'Last known spendable balance',exact:true})).toBeVisible()
  await expect(panel).toContainText('1.234567890123 XMR')
  await expect(panel.getByRole('button',{name:'Receive Monero',exact:true})).toBeDisabled()
})
