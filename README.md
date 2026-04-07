# AList + Bore 便携网盘一体化工具

AList + Bore 便携网盘一体化工具，集成了 AList 文件服务和 Bore 内网穿透功能，提供一站式的文件管理和外网访问解决方案。

## 功能特点

- 📁 集成 AList 文件服务，支持多种存储类型
- 🌐 内置 Bore 内网穿透，自动生成外网访问地址
- 🔑 密码自动保存和管理
- 🎨 美观的 Catppuccin Mocha 主题界面
- 🚀 单文件启动，无依赖
- 📱 系统托盘支持
- 🔄 服务状态实时监控

## 技术栈

- Rust 1.75+
- Tauri v1.5
- HTML + CSS + JavaScript (原生)

## 构建

### 本地构建

1. 安装 Rust 和 Node.js
2. 下载 AList 和 Bore 二进制文件到 `resources/` 目录
3. 运行 `npm install`
4. 运行 `npm run build`

### GitHub Actions 构建

项目配置了 GitHub Actions 自动构建流程，每次推送代码到 main 分支时会自动构建并上传构建产物。

## 使用

1. 双击 `AListBore.exe` 启动应用
2. 等待服务启动完成（约 5-10 秒）
3. 使用界面显示的管理员密码登录本地管理页面
4. 添加存储并开始使用

## 安全建议

- 首次登录后立即修改密码
- 定期备份重要数据
- 不要在公共网络环境下使用默认密码
- 如需长期稳定访问，建议使用自己的服务器搭建 bore 服务

## 许可证

MIT
