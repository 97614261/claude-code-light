import { MsEdgeTTS, OUTPUT_FORMAT } from "msedge-tts";
import { createWriteStream } from "node:fs";
import { pipeline } from "node:stream/promises";

// 颜色语义（A 方案，符合交通灯标准）：
//   红 = 等用户回应（停下来看）
//   黄 = Claude 在思考/执行（运行中）
//   绿 = 完成
const lines = {
  red:    "你看一下",
  yellow: "让我想想",
  green:  "好啦",
};

const tts = new MsEdgeTTS({ enableLogger: false });
await tts.setMetadata("zh-CN-XiaoxiaoNeural", OUTPUT_FORMAT.AUDIO_24KHZ_48KBITRATE_MONO_MP3);

for (const [name, text] of Object.entries(lines)) {
  const path = `src-tauri/sounds/${name}.mp3`;
  const { audioStream } = await tts.toStream(text);
  await pipeline(audioStream, createWriteStream(path));
  console.log(`${name}.wav <- "${text}"`);
}
await tts.close();
console.log("Done");
