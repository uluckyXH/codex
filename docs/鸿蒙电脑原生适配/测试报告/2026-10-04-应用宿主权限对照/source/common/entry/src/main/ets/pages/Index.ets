@Entry
@Component
struct Index {
  build() {
    Column() {
      Text('Codex 原生调试宿主').fontSize(28)
      Text('正在验证应用私有目录与原生命令运行能力').fontSize(18)
    }.width('100%').height('100%').justifyContent(FlexAlign.Center)
  }
}
