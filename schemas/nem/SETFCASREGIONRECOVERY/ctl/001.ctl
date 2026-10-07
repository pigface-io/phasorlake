-- ************************************************************ 
-- Title:	PUBLIC_DVD_SETFCASREGIONRECOVERY_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS SETFCASREGIONRECOVERY owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_SETFCASREGIONRECOVERY_201501010000.csv  
APPEND INTO TABLE SETFCASREGIONRECOVERY  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSIONNO FLOAT EXTERNAL, 
BIDTYPE CHAR(10), 
REGIONID CHAR(10), 
PERIODID FLOAT EXTERNAL, 
GENERATORREGIONENERGY FLOAT EXTERNAL, 
CUSTOMERREGIONENERGY FLOAT EXTERNAL, 
REGIONRECOVERY FLOAT EXTERNAL, 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
