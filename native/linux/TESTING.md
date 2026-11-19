# Ubuntu 验证入口

平台固定为Ubuntu24.04 amd64、GNOME Xorg、IBus1.5；实际系统、源码和制品哈希以交付记录为准。命令行检查通过后仍须安装deb，在真实GTK窗口完成选词、安全输入、焦点和学习连续性检查。

核心需要用户拥有的运行目录直接父目录；Linux默认`/tmp`归root，不能直接作为创建私有学习叶目录的父目录。测试先在项目或用户目录创建私有临时根，使用`TMPDIR=<该根>`运行。不要放宽核心的目录归属、符号链接或私有文件校验来迁就测试。核心库不提交Cargo.lock；可从已锁定宿主使用的缓存离线生成库测试锁文件，记录哈希后用`--locked`。

- `cargo test --manifest-path native/host/Cargo.toml --lib --bins`：共享宿主和桥接单元检查。
- `python3 native/linux/testtools/test_keys.py`、`test_messages.py`：按键、退格重复边界、UTF8光标、安全提示及私密错误不回显。
- `cargo test --manifest-path native/core/agent-ime-core/Cargo.toml --lib -- --test-threads=1`：新平台核心回归；固定全量词库路径由`Q12_PINYIN_SOURCE`明确指定。
- 核心公开接口测试名称为`--test sdk_public`；文档检查另用`--doc`，不能与`--test`混在一条cargo命令。
- `python3 native/linux/testtools/test_bridge_learning.py <桥接可执行文件> <含Rime的资源目录> <尚不存在的新验收目录>`：用真实桥接/核心验证普通输入→安全状态和关闭管道退出时的已确认学习，私密键不进入学习。
- `acceptance_probe.py <新验收目录>`：只提供合成GTK控件的重置、焦点和读回；实际文字由另一个控制器通过XTest按键/鼠标注入，注入前核对前台窗口ID。此程序不能给普通控件直接填入预期结果来冒充输入法上屏。

失败原始日志保留。只重跑失败或受修改影响的目标，不重复已通过的全量结果。内部rc1/rc2等候选与正式包分别保留来源和哈希，升级、回退、卸载重装和数据连续性另做实测。
