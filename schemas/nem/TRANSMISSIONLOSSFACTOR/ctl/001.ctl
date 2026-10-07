-- ************************************************************ 
-- Title:	PUBLIC_DVD_TRANSMISSIONLOSSFACTOR_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS TRANSMISSIONLOSSFACTOR owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_TRANSMISSIONLOSSFACTOR_201501010000.csv  
APPEND INTO TABLE TRANSMISSIONLOSSFACTOR  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
TRANSMISSIONLOSSFACTOR FLOAT EXTERNAL, 
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSIONNO FLOAT EXTERNAL, 
CONNECTIONPOINTID CHAR(10), 
REGIONID CHAR(10), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss", 
SECONDARY_TLF FLOAT EXTERNAL) 
-- End of Script 
