-- ************************************************************ 
-- Title:	PUBLIC_DVD_GENERICCONSTRAINTRHS_201501.ctl 
-- ************************************************************  
-- Project : NEMMCO MONTHLY DVD RELEASE 
-- Purpose : Control File to load monthly csv file 
-- Target Application  : Monthly DVD Release 
-- Application Version : 1 
-- Visibility Id : PUBLIC 
-- 
-- ************************************************************ 
-- Special Instructions : 1. USER TO CONNECT AS GENERICCONSTRAINTRHS owner 
-- 
-- ************************************************************ 
-- History : 
--      MMS Data Model Version   Template Generated On	
--      ======================   ======================	
--      PUBLIC			       v4.23	
-- ************************************************************ 
LOAD DATA 
INFILE PUBLIC_DVD_GENERICCONSTRAINTRHS_201501010000.csv  
APPEND INTO TABLE GENERICCONSTRAINTRHS  
WHEN (1:1) = 'D'  
FIELDS TERMINATED BY ','  
OPTIONALLY ENCLOSED BY '"'  
TRAILING NULLCOLS	
(row_type FILLER, 
report_type FILLER, 
report_subtype FILLER, 
report_version FILLER, 
GENCONID CHAR(20), 
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss", 
VERSIONNO FLOAT EXTERNAL, 
SCOPE CHAR(2), 
TERMID FLOAT EXTERNAL, 
GROUPID FLOAT EXTERNAL, 
SPD_ID CHAR(21), 
SPD_TYPE CHAR(1), 
FACTOR FLOAT EXTERNAL, 
OPERATION CHAR(10), 
DEFAULTVALUE FLOAT EXTERNAL, 
PARAMETERTERM1 CHAR(12), 
PARAMETERTERM2 CHAR(12), 
PARAMETERTERM3 CHAR(12), 
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss") 
-- End of Script 
