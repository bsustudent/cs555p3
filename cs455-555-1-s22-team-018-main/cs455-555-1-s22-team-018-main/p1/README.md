Project number: p1  
Project title: Chat Server  
Team members: Colin Reeder, Yoonsu Ra  
Course number: CS 455/555  
Course title: Distributed Systems  
Semester: Spring  
Year: 2022  

Guide through code:  
server/src/main.rs-  
-Starts up the server  
-Listens for clients joining.  
-Handles receiving messages from clients and sending to other clients in same channel as original client.  
-Handles receiving different commands from the client (/join, /nick, etc)
-Shutdowns after 5mins of idle time
 
client/src/main.rs-  
-Connects to the server  
-Sends messages to the server  
-Receives messages from other clients through the server  
-Sends different commands to server (/join, /nick, etc)  

types/src/lib.rs-  
-packets from server to client  
-packets from client to server  
-list channel info  
 
*each of the .rs is associated with a Cargo.toml that is used to refer to the dependencies
 
How to build and run server:  
Prerequisites-
1. Rust installed.
2. Visual Studio

Running Server-
1. Change to project directory.
2. Change to server directory.
3. Run command "cargo run".

Running Client- (on seperate console)
1. Change to project directory.
2. Change to server directory.
3. Run command "cargo run".


How to test: (through client console)
1. /connect localhost:5005
2. /join aroom
3. Type any message and enter
4. Server commands can be seen with "/help"
  
Observations/reflection:  
  Yoonsu- First time using Rust so I needed a lot of assistance with understanding the syntax and other nuances.
  
  
Roles:
 Server  
 I. 5m idle shutdown *Yoonsu  
 II. shutdown hook *Yoonsu  
 III. sending messages to all clients in channel *Colin  
 IV. server up time *Yoonsu  
 V. port command arguements *Colin  
 VI. multi channel, multi-threaded *Colin  
 Client  
 I. connect *Colin  
 II. nick *Colin  
 III. list *Colin  
 IV. join channel *Colin  
 V. leave *Colin  
 VI. quit *Colin  
 VII. help *Colin  
  a. list of commands *Colin  
 VIII. stats *Yoonsu  
  a. # of clients *Yoonsu  
  b. time server has been running *Yoonsu  
 IX. sending messages to server *Colin  
  
 javadocs *Colin  
 README.md *Yoonsu  
 video (https://drive.google.com/file/d/1xFT8JeUMkzNHznL1YA3M0GzoE283f49g/view?usp=sharing)  *Colin *Yoonsu  
